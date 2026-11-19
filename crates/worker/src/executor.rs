//! Action Executor Module
//!
//! Coordinates the execution of actions by managing the runtime,
//! loading action data, preparing execution context, and collecting results.
//!
//! ## Runtime Version Selection
//!
//! When an action declares a `runtime_version_constraint` (e.g., `">=3.12"`),
//! the executor queries the `runtime_version` table for all versions of the
//! action's runtime and uses [`select_best_version`] to pick the highest
//! available version satisfying the constraint. The selected version's
//! `execution_config` is passed through the `ExecutionContext` as an override
//! so the `ProcessRuntime` uses version-specific interpreter binaries,
//! environment commands, etc.

use attune_common::artifact_transport::{
    resolve_checked_path, sync_local_file_to_transport, ArtifactFileTransport,
    ValidatedRelativePath,
};
#[cfg(test)]
use attune_common::auth::jwt::generate_execution_token_with_permission_sets_and_standard_access;
use attune_common::auth::jwt::JwtConfig;
use attune_common::error::{Error, Result};
use attune_common::metadata_cache::MetadataCache;
use attune_common::models::runtime::RuntimeExecutionConfig;
use attune_common::models::{
    enums::{ArtifactType, ArtifactVisibility, LogStreamBackend, OwnerType, RetentionPolicyType},
    runtime::Runtime as RuntimeModel,
    runtime::RuntimeVersion as RuntimeVersionModel,
    Action, Execution, ExecutionStatus, Worker,
};
use attune_common::repositories::action::ActionRepository;
use attune_common::repositories::artifact::{
    classify_artifact, default_content_type_for_artifact, ArtifactRepository,
    ArtifactVersionRepository, CreateArtifactInput, UpdateArtifactInput,
};
use attune_common::repositories::execution::{ExecutionRepository, ExecutionTransition};
use attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository;
use attune_common::repositories::runtime::WorkerRepository;
use attune_common::repositories::runtime::SELECT_COLUMNS as RUNTIME_SELECT_COLUMNS;
use attune_common::repositories::runtime_version::RuntimeVersionRepository;
use attune_common::repositories::schema_contract::ActionSchemaBinding;
use attune_common::repositories::{FindById, FindByRef, Update};
use attune_common::runtime_detection::normalize_runtime_name;
use attune_common::secret_values::{prepare_secret_values, ENTITY_EXECUTION_RESULT};
use attune_common::version_matching::{matches_constraint, select_best_version};
use std::path::PathBuf as StdPathBuf;

use serde_json::Value as JsonValue;
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::time::Duration;
use tracing::{debug, error, info, warn};

use crate::artifacts::ArtifactManager;
use crate::runtime::{BoundedLogFileWriter, ExecutionContext, ExecutionResult, RuntimeRegistry};
use crate::secrets::SecretManager;

/// A persisted terminal row and the process outcome. Preparation failures can
/// have a durable Failed row even though no process result exists.
pub(crate) struct ExecutionOutcome {
    pub transition: ExecutionTransition,
    pub result: Result<ExecutionResult>,
}

/// Action executor that orchestrates execution flow
pub struct ActionExecutor {
    #[cfg(unix)]
    log_delivery: tokio::sync::OnceCell<Arc<crate::log_delivery::LogDeliveryManager>>,
    #[cfg(unix)]
    log_custody_owner:
        tokio::sync::Mutex<Option<(i64, crate::log_delivery_spool::UnboundLogDeliverySpool)>>,
    pool: PgPool,
    child_environment: attune_common::child_process_environment::ChildProcessEnvironment,
    runtime_registry: RuntimeRegistry,
    artifact_manager: ArtifactManager,
    secret_manager: SecretManager,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
    mirror_runtime_stdout_to_stdio: bool,
    mirror_runtime_stderr_to_stdio: bool,
    worker_id: AtomicI64,
    worker_name: String,
    worker_instance: uuid::Uuid,
    /// Registration ownership is distinct from the log-delivery bearer instance.
    process_incarnation: std::sync::OnceLock<uuid::Uuid>,
    log_segment_config: attune_common::log_stream::SegmentedLogConfig,
    execution_log_retention_policy: RetentionPolicyType,
    execution_log_retention_limit: i32,
    packs_base_dir: PathBuf,
    artifacts_dir: PathBuf,
    runtime_envs_dir: PathBuf,
    api_url: String,
    jwt_config: JwtConfig,
    worker_tokens: Option<Arc<attune_common::auth::WorkerTokenProvider>>,
    recorded_readiness: crate::recorded_credentials::RecordedConsumerReadiness,
    /// Transport abstraction for artifact file content.
    transport: Arc<dyn ArtifactFileTransport>,
    /// Concrete factual transfer/removal effects. Domain admission and
    /// completion remain in this executor; this is never a workload service.
    native_backend: Option<attune_workload_backend::configuration::NativeBackend>,
    native_preparation: Option<NativePreparation>,
    action_cache_by_id: Arc<MetadataCache<String, Action>>,
    action_cache_by_ref: Arc<MetadataCache<String, Action>>,
    runtime_cache_by_id: Arc<MetadataCache<String, RuntimeModel>>,
    runtime_cache_by_ref: Arc<MetadataCache<String, RuntimeModel>>,
    runtime_versions_cache_by_runtime_id: Arc<MetadataCache<String, Vec<RuntimeVersionModel>>>,
}

/// Operator configuration and the already-installed public Runner. All action
/// selection remains in the existing trusted executor and retained release.
struct NativePreparation {
    config: attune_common::workload_config::WorkloadConfig,
    cache: Arc<attune_workload_backend::cache::CacheStore>,
    runner: attune_common::workload_launch::PinnedLaunchInput,
}

/// Provider exit is durable evidence, not completion policy. This wrapper
/// belongs to the existing trusted executor; it never enters a payload Pod.
struct RecordingProcessControl {
    inner: Arc<dyn attune_common::isolated_process::ProcessControl>,
    pool: PgPool,
    intent: uuid::Uuid,
    resource: attune_common::isolated_process::ResourceIdentity,
}
#[async_trait::async_trait]
impl attune_common::isolated_process::ProcessControl for RecordingProcessControl {
    async fn write_stdin_once(
        &self,
        payload: attune_common::workload_stdin::LaunchStdinPayload,
    ) -> Result<()> {
        self.inner.write_stdin_once(payload).await
    }
    async fn wait(&self) -> Result<attune_common::workload_launch::LaunchExitOutcome> {
        let outcome = self.inner.wait().await?;
        attune_common::repositories::execution_process::ExecutionProcessRepository::record_exit(
            &self.pool,
            self.intent,
            &self.resource,
            outcome,
        )
        .await?;
        Ok(outcome)
    }
    async fn stop(
        &self,
        reason: attune_common::workload_launch::LaunchStopReason,
    ) -> Result<attune_common::workload_launch::LaunchExitOutcome> {
        // Stop the exact owned payload even while DB fencing is waiting or
        // unavailable. Join both owners; physical termination alone is never a
        // durable stop/completion acknowledgement.
        let (fenced, stopped) = tokio::join!(
            attune_common::repositories::execution_process::ExecutionProcessRepository::request_stop(&self.pool, self.intent),
            self.inner.stop(reason),
        );
        let outcome = stopped?;
        let recorded=attune_common::repositories::execution_process::ExecutionProcessRepository::record_exit(
            &self.pool,self.intent,&self.resource,outcome,
        ).await;
        fenced?;
        recorded?;
        Ok(outcome)
    }
}

use tokio_util::sync::CancellationToken;

/// Normalize a server bind address into a connectable URL.
///
/// When the server binds to `0.0.0.0` (all interfaces) or `::` (IPv6 any),
/// we substitute `127.0.0.1` so that actions running on the same host can
/// reach the API.
fn normalize_api_url(raw_url: &str) -> String {
    raw_url
        .replace("://0.0.0.0", "://127.0.0.1")
        .replace("://[::]", "://127.0.0.1")
}

/// Default retention policy for per-execution stdout/stderr log artifacts.
/// The worker service passes configured values into `ActionExecutor::new`.
const DEFAULT_LOG_ARTIFACT_RETENTION_POLICY: RetentionPolicyType = RetentionPolicyType::Days;
const DEFAULT_LOG_ARTIFACT_RETENTION_LIMIT: i32 = 7;
const ACTION_CACHE_TTL: Duration = Duration::from_secs(30);
const ACTION_CACHE_MAX_ENTRIES: usize = 2048;
const RUNTIME_CACHE_TTL: Duration = Duration::from_secs(60);
const RUNTIME_CACHE_MAX_ENTRIES: usize = 1024;
const RUNTIME_VERSIONS_CACHE_TTL: Duration = Duration::from_secs(60);
const RUNTIME_VERSIONS_CACHE_MAX_ENTRIES: usize = 1024;

/// Resolve the identity to embed in the execution-scoped API token (`sub`
/// claim).
///
/// Execution API authority always needs an explicitly attributed identity.
fn resolve_execution_identity(executor: Option<i64>, execution_id: i64) -> Result<i64> {
    executor.ok_or_else(|| {
        Error::PermissionDenied(format!(
            "Execution {execution_id} has no executor identity for API access"
        ))
    })
}

#[cfg(test)]
fn generate_runtime_api_token(
    identity_id: i64,
    execution_id: i64,
    action_ref: &str,
    jwt_config: &JwtConfig,
    token_ttl: i64,
    permission_set_refs: &[String],
    standard_access_action_refs: &[String],
) -> std::result::Result<Option<String>, attune_common::auth::jwt::JwtError> {
    if permission_set_refs.is_empty() {
        return Ok(None);
    }
    generate_execution_token_with_permission_sets_and_standard_access(
        identity_id,
        execution_id,
        action_ref,
        jwt_config,
        Some(token_ttl),
        permission_set_refs,
        standard_access_action_refs,
    )
    .map(Some)
}

fn emit_runtime_log_truncation_events(execution_id: i64, result: &ExecutionResult) {
    if result.stdout_truncated {
        info!(
            execution_id,
            stream = "stdout",
            bytes_truncated = result.stdout_bytes_truncated,
            "Runtime log truncated"
        );
    }
    if result.stderr_truncated {
        info!(
            execution_id,
            stream = "stderr",
            bytes_truncated = result.stderr_bytes_truncated,
            "Runtime log truncated"
        );
    }
    if result.logs_incomplete {
        warn!(
            execution_id,
            "Runtime logs were not fully drained and sealed before finalization"
        );
    }
}

fn record_runtime_log_capture(
    result_data: &mut serde_json::Value,
    result: &ExecutionResult,
    allocation: Option<(bool, bool)>,
) {
    if result.stdout_truncated {
        result_data["stdout_truncated"] = serde_json::json!(true);
        result_data["stdout_bytes_truncated"] = serde_json::json!(result.stdout_bytes_truncated);
    }
    if result.stderr_truncated {
        result_data["stderr_truncated"] = serde_json::json!(true);
        result_data["stderr_bytes_truncated"] = serde_json::json!(result.stderr_bytes_truncated);
    }
    // Keep the bounded original diagnostic available for channel classification.
    if !result.stderr.is_empty() {
        result_data["stderr"] = serde_json::json!(result.stderr);
    }
    // Allocation evidence proves whether bytes arrived independently of bounded
    // captures and parsed structured output.
    let state = |allocated: Option<bool>, incomplete: bool| {
        if incomplete {
            "incomplete"
        } else {
            match allocated {
                Some(true) => "captured",
                Some(false) => "empty",
                None => "unknown",
            }
        }
    };
    result_data["runtime_log_capture"] = serde_json::json!({
        "stdout": state(allocation.map(|(stdout, _)| stdout), result.log_capture_incomplete.map(|(stdout,_)|stdout).unwrap_or(result.logs_incomplete)),
        "stderr": state(allocation.map(|(_, stderr)| stderr), result.log_capture_incomplete.map(|(_,stderr)|stderr).unwrap_or(result.logs_incomplete)),
    });
    if result.logs_incomplete {
        result_data["logs_incomplete"] = serde_json::json!(true);
    }
}

/// Resolve the effective execution timeout (in seconds) for a worker run.
///
/// The execution timeout is normally **snapshotted** onto `execution.timeout_seconds`
/// at creation time. This helper reads that snapshot first and only applies
/// defensive fallback resolution for legacy / incomplete rows that predate the
/// snapshot column:
///
/// `execution.timeout_seconds` -> `workflow_task.timeout_seconds`
///   -> `action.timeout_seconds` -> app-level `default_execution_timeout_seconds`.
///
/// The returned value is always a positive number of seconds.
pub(crate) fn resolve_execution_timeout_seconds(execution: &Execution, action: &Action) -> u64 {
    resolve_timeout_seconds_inner(
        execution.timeout_seconds,
        execution
            .workflow_task
            .as_ref()
            .and_then(|metadata| metadata.timeout_seconds),
        action.timeout_seconds,
        attune_common::config::app_default_execution_timeout_seconds(),
    )
}

/// Pure resolution of the effective execution timeout (seconds).
///
/// Precedence: snapshotted execution timeout -> workflow task timeout ->
/// action default -> app-config default. Non-positive or out-of-range values
/// are ignored at each tier so a legacy/incomplete row still falls through to
/// the next defensive default.
fn resolve_timeout_seconds_inner(
    snapshot: Option<i32>,
    workflow_task: Option<i32>,
    action: Option<i32>,
    app_default: u64,
) -> u64 {
    for candidate in [snapshot, workflow_task, action] {
        if let Some(seconds) = candidate
            .and_then(|t| u64::try_from(t).ok())
            .filter(|t| *t > 0)
        {
            return seconds;
        }
    }
    app_default
}

#[derive(Clone, Copy)]
enum ExecutionLogArtifactStream {
    Stdout,
    Stderr,
}

impl ExecutionLogArtifactStream {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

/// Execution-scoped immutable contract and the authorized input paths observed
/// while restoring parameters. Runtime reads/origins remain live at completion.
struct ExecutionPreparation {
    process_intent: Option<uuid::Uuid>,
    binding: ActionSchemaBinding,
    parameter_secret_paths: Option<Vec<String>>,
    input_snapshot: Option<attune_common::repositories::execution_input::ExecutionInputSnapshot>,
    stdout_path: Arc<OnceLock<PathBuf>>,
    stderr_path: Arc<OnceLock<PathBuf>>,
}

impl ExecutionPreparation {
    fn log_allocation(&self) -> (bool, bool) {
        (
            self.stdout_path.get().is_some(),
            self.stderr_path.get().is_some(),
        )
    }
}

struct ExecutionLogArtifacts {
    stdout_writer: BoundedLogFileWriter,
    stderr_writer: BoundedLogFileWriter,
}

#[derive(Clone, Copy)]
struct LogAllocationSettings {
    policy: RetentionPolicyType,
    limit: i32,
    segment_bytes: u64,
    flush_interval_ms: u64,
}

fn inline_action_source(
    entrypoint: &str,
    is_adhoc: bool,
    runtime_name: Option<&str>,
    config: Option<&RuntimeExecutionConfig>,
    has_file: bool,
) -> Option<String> {
    (!has_file
        && (runtime_name == Some("shell")
            || (is_adhoc && config.is_some_and(|config| config.lifecycle.is_some()))))
    .then(|| entrypoint.to_owned())
}

impl ActionExecutor {
    pub fn with_native_execution(
        mut self,
        config: attune_common::workload_config::WorkloadConfig,
        configured: attune_workload_backend::configuration::ConfiguredBackend,
    ) -> Result<Self> {
        config.validate()?;
        if configured.runner != config.helper {
            return Err(Error::configuration(
                "Native Runner pin does not match operator configuration",
            ));
        }
        self.native_backend = Some(configured.backend);
        self.native_preparation = Some(NativePreparation {
            config,
            cache: configured.cache,
            runner: configured.runner,
        });
        Ok(self)
    }

    /// Integration startup must prove all consumers are ready before enabling
    /// recorded issuance. No production caller enables this yet.
    pub fn with_recorded_readiness(
        mut self,
        readiness: crate::recorded_credentials::RecordedConsumerReadiness,
    ) -> Result<Self> {
        readiness.require_ready()?;
        self.recorded_readiness = readiness;
        Ok(self)
    }

    pub fn with_child_environment(
        mut self,
        environment: attune_common::child_process_environment::ChildProcessEnvironment,
    ) -> Self {
        self.child_environment = environment;
        self
    }

    /// Create a new action executor
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        runtime_registry: RuntimeRegistry,
        artifact_manager: ArtifactManager,
        secret_manager: SecretManager,
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
        mirror_runtime_stdout_to_stdio: bool,
        mirror_runtime_stderr_to_stdio: bool,
        worker_name: String,
        worker_instance: uuid::Uuid,
        log_segment_config: attune_common::log_stream::SegmentedLogConfig,
        execution_log_retention_policy: Option<RetentionPolicyType>,
        execution_log_retention_limit: Option<i32>,
        packs_base_dir: PathBuf,
        artifacts_dir: PathBuf,
        runtime_envs_dir: PathBuf,
        api_url: String,
        jwt_config: JwtConfig,
        transport: Arc<dyn ArtifactFileTransport>,
    ) -> Self {
        let api_url = normalize_api_url(&api_url);
        Self {
            #[cfg(unix)]
            log_delivery: tokio::sync::OnceCell::new(),
            #[cfg(unix)]
            log_custody_owner: tokio::sync::Mutex::new(None),
            pool,
            child_environment: Default::default(),
            runtime_registry,
            artifact_manager,
            secret_manager,
            max_stdout_bytes,
            max_stderr_bytes,
            mirror_runtime_stdout_to_stdio,
            mirror_runtime_stderr_to_stdio,
            worker_id: AtomicI64::new(0),
            worker_name,
            worker_instance,
            process_incarnation: std::sync::OnceLock::new(),
            log_segment_config,
            execution_log_retention_policy: execution_log_retention_policy
                .unwrap_or(DEFAULT_LOG_ARTIFACT_RETENTION_POLICY),
            execution_log_retention_limit: execution_log_retention_limit
                .unwrap_or(DEFAULT_LOG_ARTIFACT_RETENTION_LIMIT),
            packs_base_dir,
            artifacts_dir,
            runtime_envs_dir,
            api_url,
            jwt_config,
            worker_tokens: None,
            recorded_readiness: Default::default(),
            transport,
            native_backend: None,
            native_preparation: None,
            action_cache_by_id: Arc::new(MetadataCache::new(
                ACTION_CACHE_TTL,
                ACTION_CACHE_MAX_ENTRIES,
            )),
            action_cache_by_ref: Arc::new(MetadataCache::new(
                ACTION_CACHE_TTL,
                ACTION_CACHE_MAX_ENTRIES,
            )),
            runtime_cache_by_id: Arc::new(MetadataCache::new(
                RUNTIME_CACHE_TTL,
                RUNTIME_CACHE_MAX_ENTRIES,
            )),
            runtime_cache_by_ref: Arc::new(MetadataCache::new(
                RUNTIME_CACHE_TTL,
                RUNTIME_CACHE_MAX_ENTRIES,
            )),
            runtime_versions_cache_by_runtime_id: Arc::new(MetadataCache::new(
                RUNTIME_VERSIONS_CACHE_TTL,
                RUNTIME_VERSIONS_CACHE_MAX_ENTRIES,
            )),
        }
    }

    pub fn with_worker_token_provider(
        mut self,
        provider: Arc<attune_common::auth::WorkerTokenProvider>,
    ) -> Self {
        self.worker_tokens = Some(provider);
        self
    }

    fn needs_log_custody(&self, config: &attune_common::config::LogDeliveryConfig) -> bool {
        cfg!(unix) && config.enabled && self.transport.transport_mode() != "volume"
    }

    /// Existing trusted startup policy, with exclusive custody preceding the
    /// only operation that can replace a live registered incarnation.
    pub(crate) async fn register_worker_with_custody(
        &self,
        registration: &mut crate::registration::WorkerRegistration,
        config: &attune_common::config::LogDeliveryConfig,
    ) -> Result<i64> {
        if self.needs_log_custody(config) {
            let worker = registration.reserve_slot().await?;
            self.acquire_log_custody(config, worker).await?;
            let registered = registration.register_reserved(worker).await;
            #[cfg(unix)]
            if registered.is_err() {
                // No capture/recovery task can exist before registration ACK.
                // Release only ownership, retaining the synchronized key/files.
                self.log_custody_owner.lock().await.take();
            }
            registered
        } else {
            registration.register().await
        }
    }

    /// Prepare a stable ID's existing custody root without granting a worker
    /// incarnation any authority. Kept for explicit owning integration fixtures
    /// too; ordinary service startup uses register_worker_with_custody above.
    pub async fn acquire_log_custody(
        &self,
        config: &attune_common::config::LogDeliveryConfig,
        worker: i64,
    ) -> Result<()> {
        #[cfg(unix)]
        if self.needs_log_custody(config) {
            if worker <= 0 || config.max_finalizers == 0 || config.max_spool_bytes == 0 {
                return Err(Error::validation(
                    "Log delivery ownership and budgets must be valid",
                ));
            }
            let mut holder = self.log_custody_owner.lock().await;
            if holder.is_some() || self.log_delivery.get().is_some() {
                return Err(Error::invalid_state("Log custody already acquired"));
            }
            let root = config
                .spool_dir
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| self.runtime_envs_dir.join(".log-delivery"))
                .join(worker.to_string());
            let max_plain = self.max_stdout_bytes.max(self.max_stderr_bytes) as u64;
            let max_payload = attune_common::runtime_read_log::maximum_encoded_bytes(max_plain)?;
            let entry_bytes = max_payload
                .checked_mul(4)
                .and_then(|v| v.checked_add(64 * 1024))
                .ok_or_else(|| Error::validation("Log spool size overflow"))?;
            let owner = crate::log_delivery_spool::LogDeliverySpool::acquire_owner(
                root,
                config.max_spool_bytes,
                entry_bytes,
                max_payload,
            )
            .await?;
            *holder = Some((worker, owner));
        }
        #[cfg(not(unix))]
        let _ = (config, worker);
        Ok(())
    }

    pub async fn start_log_delivery(
        &self,
        config: attune_common::config::LogDeliveryConfig,
    ) -> Result<()> {
        #[cfg(unix)]
        if self.needs_log_custody(&config) {
            let worker = self.worker_id.load(Ordering::Acquire);
            let (reserved, owner) =
                self.log_custody_owner.lock().await.take().ok_or_else(|| {
                    Error::invalid_state("Log custody must precede worker registration")
                })?;
            if reserved != worker {
                return Err(Error::PermissionDenied(
                    "Log custody registration changed".into(),
                ));
            }
            let max_plain = self.max_stdout_bytes.max(self.max_stderr_bytes) as u64;
            let max_payload = attune_common::runtime_read_log::maximum_encoded_bytes(max_plain)?;
            let manager = crate::log_delivery::LogDeliveryManager::open_owned(
                self.pool.clone(),
                owner.bind_worker(worker)?,
                worker,
                self.worker_instance,
                self.worker_tokens.clone().ok_or_else(|| {
                    Error::invalid_state("Log delivery worker credential provider unavailable")
                })?,
                self.api_url.clone(),
                config,
                max_payload,
                self.log_segment_config.max_segment_bytes,
            )
            .await?;
            self.log_delivery
                .set(manager)
                .map_err(|_| Error::invalid_state("Log delivery started twice"))?;
        }
        #[cfg(not(unix))]
        let _ = config;
        Ok(())
    }
    pub async fn drain_log_delivery(&self) {
        #[cfg(unix)]
        if let Some(manager) = self.log_delivery.get() {
            manager.drain().await;
        }
    }
    /// The caller must first join every attempt/capture producer. Retained
    /// custody remains durable; a new manager may recover before its deadline.
    pub async fn stop_log_delivery(&self) {
        #[cfg(unix)]
        if let Some(manager) = self.log_delivery.get() {
            manager.stop_and_join().await;
        }
    }
    /// Bind exactly once to this service's acknowledged worker registration.
    /// A replacement incarnation requires a new owner/executor, never rebinding
    /// an old process that may still retain attempts or private custody.
    pub fn set_process_incarnation(&self, incarnation: uuid::Uuid) -> Result<()> {
        if incarnation.is_nil() {
            return Err(Error::validation("Missing worker process incarnation"));
        }
        if self.process_incarnation.set(incarnation).is_err()
            && self.process_incarnation.get() != Some(&incarnation)
        {
            return Err(Error::PermissionDenied(
                "Worker process incarnation already bound".into(),
            ));
        }
        Ok(())
    }
    pub fn set_worker_id(&self, worker_id: i64) {
        self.worker_id.store(worker_id, Ordering::Release);
    }

    pub async fn invalidate_action_cache(&self, action_id: Option<i64>, action_ref: Option<&str>) {
        let mut evicted = 0usize;
        if let Some(id) = action_id {
            let key = Self::action_id_cache_key(id);
            if self.action_cache_by_id.invalidate_key(&key).await {
                evicted += 1;
            }
        }
        if let Some(action_ref) = action_ref {
            let key = Self::action_ref_cache_key(action_ref);
            if self.action_cache_by_ref.invalidate_key(&key).await {
                evicted += 1;
            }
        }
        debug!(
            entity = "action",
            operation = "invalidate",
            evicted,
            "worker metadata cache invalidation"
        );
    }

    pub async fn invalidate_runtime_cache(
        &self,
        runtime_id: Option<i64>,
        runtime_ref: Option<&str>,
    ) {
        let mut evicted = 0usize;
        if let Some(id) = runtime_id {
            let id_key = Self::runtime_id_cache_key(id);
            if self.runtime_cache_by_id.invalidate_key(&id_key).await {
                evicted += 1;
            }
            if self
                .runtime_versions_cache_by_runtime_id
                .invalidate_key(&id_key)
                .await
            {
                evicted += 1;
            }
        }
        if let Some(runtime_ref) = runtime_ref {
            let ref_key = Self::runtime_ref_cache_key(runtime_ref);
            if self.runtime_cache_by_ref.invalidate_key(&ref_key).await {
                evicted += 1;
            }
        }
        debug!(
            entity = "runtime",
            operation = "invalidate",
            evicted,
            "worker metadata cache invalidation"
        );
    }

    /// Execute an action for the given execution
    pub async fn execute(&self, execution_id: i64) -> Result<ExecutionResult> {
        self.execute_with_cancel(execution_id, CancellationToken::new())
            .await
    }

    /// Execute an action for the given execution, with cancellation support.
    ///
    /// When the `cancel_token` is triggered, the running process receives
    /// SIGTERM → SIGKILL with a short grace period.
    pub async fn execute_with_cancel(
        &self,
        execution_id: i64,
        cancel_token: CancellationToken,
    ) -> Result<ExecutionResult> {
        self.execute_with_cancel_outcome(execution_id, cancel_token)
            .await?
            .result
    }

    pub(crate) async fn execute_with_cancel_outcome(
        &self,
        execution_id: i64,
        cancel_token: CancellationToken,
    ) -> Result<ExecutionOutcome> {
        let transition = self.start_execution(execution_id).await?;
        self.execute_started_outcome(transition.execution, cancel_token)
            .await
    }

    pub(crate) async fn start_execution(&self, execution_id: i64) -> Result<ExecutionTransition> {
        let worker = self.worker_id.load(Ordering::Acquire);
        let mut tx = self.pool.begin().await?;
        let authority = attune_common::repositories::execution_runtime_admission::ExecutionAdmissionRepository::existing_authority(&mut tx, execution_id).await?;
        if authority == attune_common::repositories::execution_runtime_admission::ExistingExecutionAuthority::Recorded {
            self.recorded_readiness.require_ready()?;
        } else {
            // Claim and attempt registration commit together. A duplicate or
            // stale delivery cannot establish another credential authority.
            ExecutionRepository::find_by_id_for_update(&mut tx, execution_id).await?
                .ok_or_else(|| Error::PermissionDenied("Execution unavailable".into()))?;
            attune_common::repositories::delegated_execution::DelegatedExecutionRepository::start(&mut tx, execution_id, worker).await?;
        }
        let transition = ExecutionRepository::transition_worker(
            &mut *tx,
            execution_id,
            Some(worker).filter(|id| *id > 0),
            ExecutionStatus::Running,
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(transition)
    }

    #[tracing::instrument(skip_all, fields(execution_id = execution.id))]
    pub(crate) async fn execute_started_outcome(
        &self,
        execution: Execution,
        cancel_token: CancellationToken,
    ) -> Result<ExecutionOutcome> {
        let isolated = if self.native_preparation.is_some() {
            let spec = self.prepare_native_process(&execution).await?;
            let launcher = self
                .native_backend
                .as_ref()
                .ok_or_else(|| Error::invalid_state("Native process effects unavailable"))?
                .launcher();
            Some((spec, launcher))
        } else {
            None
        };
        self.execute_started_with_effect(execution, cancel_token, isolated)
            .await
    }

    /// The same path used by the trusted service, with no caller-supplied plan.
    /// Exposed for the owning provider fixture; configuration is manager-only.
    pub async fn execute_started_native(
        &self,
        execution: Execution,
        cancel_token: CancellationToken,
    ) -> Result<ExecutionResult> {
        if self.native_preparation.is_none() {
            return Err(Error::configuration("Native execution is not configured"));
        }
        self.execute_started_outcome(execution, cancel_token)
            .await?
            .result
    }

    /// The existing trusted loop retains preparation, capture, custody and
    /// completion. Only native Pod/container effects are replaced. The plan is
    /// supplied by trusted preparation, never a workload/API request.
    pub async fn execute_started_isolated(
        &self,
        execution: Execution,
        cancel_token: CancellationToken,
        spec: attune_common::isolated_process::ProcessSpec,
        launcher: Arc<dyn attune_common::isolated_process::ProcessLauncher>,
    ) -> Result<ExecutionResult> {
        self.execute_started_with_effect(execution, cancel_token, Some((spec, launcher)))
            .await?
            .result
    }

    async fn execute_started_with_effect(
        &self,
        execution: Execution,
        cancel_token: CancellationToken,
        isolated: Option<(
            attune_common::isolated_process::ProcessSpec,
            Arc<dyn attune_common::isolated_process::ProcessLauncher>,
        )>,
    ) -> Result<ExecutionOutcome> {
        let execution_id = execution.id;
        let isolated_attempt = if let Some((spec, _)) = &isolated {
            match &spec.target {
                attune_common::workload_launch::LaunchTarget::Execution {
                    execution,
                    worker,
                    worker_instance,
                    attempt,
                } if *execution == execution_id
                    && *worker == self.worker_id.load(Ordering::Acquire)
                    && self.process_incarnation.get() == Some(worker_instance)
                    && !attempt.is_nil() =>
                {
                    Some(*attempt)
                }
                _ => {
                    return Err(Error::PermissionDenied(
                        "Isolated action target unavailable".into(),
                    ))
                }
            }
        } else {
            None
        };
        info!("Starting execution: {}", execution_id);

        // Load action from database
        let action = match self.load_action(&execution).await {
            Ok(action) => action,
            Err(error) => {
                let transition = self
                    .fail_before_preparation(execution_id, "Failed to load pinned action")
                    .await?;
                return Ok(ExecutionOutcome {
                    transition,
                    result: Err(error),
                });
            }
        };

        let preparation_started = std::time::Instant::now();
        let mut preparation = ExecutionPreparation {
            process_intent: isolated.as_ref().map(|(spec, _)| spec.intent),
            binding: match self.schema_binding(execution_id).await {
                Ok(binding) => binding,
                Err(error) => {
                    let transition = self
                        .fail_before_preparation(
                            execution_id,
                            "Execution schema binding unavailable",
                        )
                        .await?;
                    return Ok(ExecutionOutcome {
                        transition,
                        result: Err(error),
                    });
                }
            },
            parameter_secret_paths: None,
            input_snapshot: None,
            stdout_path: Arc::new(OnceLock::new()),
            stderr_path: Arc::new(OnceLock::new()),
        };
        // The immutable contract is shared by validation, capture and redaction.
        let mut context = match self
            .prepare_execution_context(&execution, &action, &mut preparation, isolated_attempt)
            .await
        {
            Ok(ctx) => ctx,
            Err(e) => {
                if isolated_attempt.is_some() {
                    return Err(e);
                }
                error!("Failed to prepare execution context: {}", e);
                let transition = self
                    .handle_execution_failure(
                        execution_id,
                        Some(&action),
                        None,
                        Some(&format!("Failed to prepare execution context: {}", e)),
                        Some(&preparation),
                    )
                    .await?;
                return Ok(ExecutionOutcome {
                    transition,
                    result: Err(e),
                });
            }
        };

        debug!(
            phase = "preparation",
            elapsed_us = preparation_started.elapsed().as_micros() as u64,
            "Worker lifecycle phase"
        );

        // Attach the cancellation token so the process executor can monitor it
        context.cancel_token = Some(cancel_token.clone());

        // Execute the action
        // Note: execute_action should rarely return Err - most failures should be
        // captured in ExecutionResult with non-zero exit codes
        let process_started = std::time::Instant::now();
        let action_result = if let Some((spec, launcher)) = isolated {
            self.execute_isolated_action(context, &preparation, spec, launcher)
                .await
        } else {
            self.execute_action(context).await
        };
        let result = match action_result {
            Ok(result) => result,
            Err(e) => {
                if isolated_attempt.is_some() {
                    return Err(e);
                }
                error!("Action execution failed catastrophically: {}", e);
                if let Err(finalize_err) = self.finalize_file_artifacts(execution_id).await {
                    warn!(
                        "Failed to finalize file-backed artifacts for execution {} after catastrophic failure: {}",
                        execution_id, finalize_err
                    );
                }
                // This should only happen for unrecoverable errors like runtime not found
                let transition = self
                    .handle_execution_failure(
                        execution_id,
                        Some(&action),
                        None,
                        Some(&format!("Action execution failed: {}", e)),
                        Some(&preparation),
                    )
                    .await?;
                return Ok(ExecutionOutcome {
                    transition,
                    result: Err(e),
                });
            }
        };

        debug!(
            phase = "process_and_capture",
            elapsed_us = process_started.elapsed().as_micros() as u64,
            "Worker lifecycle phase"
        );
        let artifacts_started = std::time::Instant::now();
        // Finalize file-backed artifacts (stat files on disk and update size_bytes)
        if let Some(intent) = preparation.process_intent {
            // Provider errors and uncertain publication retain private staging
            // and the owning attempt. Never route isolated files through a
            // manager-local path or acknowledge them as empty/missing.
            self.finalize_isolated_file_artifacts(execution_id, intent)
                .await?;
        } else if let Err(e) = self.finalize_file_artifacts(execution_id).await {
            warn!(
                "Failed to finalize file-backed artifacts for execution {}: {}",
                execution_id, e
            );
            // Don't fail the execution just because artifact finalization failed
        }

        debug!(
            phase = "ordinary_artifact_settlement",
            elapsed_us = artifacts_started.elapsed().as_micros() as u64,
            "Worker lifecycle phase"
        );
        let terminal_started = std::time::Instant::now();
        // Update execution with result
        let is_success = result.is_success();
        debug!(
            execution_id,
            exit_code = result.exit_code,
            duration_ms = result.duration_ms,
            is_success,
            timed_out = result.timed_out,
            "Execution result received"
        );

        let was_cancelled = cancel_token.is_cancelled()
            || result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("cancelled"));

        let transition = if was_cancelled {
            self.handle_execution_cancelled(execution_id, &action, &result, &preparation)
                .await?
        } else if result.timed_out {
            self.handle_execution_timeout(execution_id, &action, &result, &preparation)
                .await?
        } else if is_success {
            self.handle_execution_success(execution_id, &action, &result, &preparation)
                .await?
        } else {
            self.handle_execution_failure(
                execution_id,
                Some(&action),
                Some(&result),
                None,
                Some(&preparation),
            )
            .await?
        };

        if let (Some(intent), Some(backend)) =
            (preparation.process_intent, self.native_backend.as_ref())
        {
            let (spec, resource) = self.retained_process(intent).await?;
            // The terminal transition and all selected file receipts committed
            // first. Logs now belong to manager-only encrypted custody (or are
            // explicitly incomplete), independently of the payload resource.
            if backend.remove_stopped(&spec, &resource).await.is_err() {
                warn!(
                    execution_id,
                    "Stopped native resource cleanup remains uncertain"
                );
            }
        }

        debug!(
            phase = "terminal_persistence",
            elapsed_us = terminal_started.elapsed().as_micros() as u64,
            "Worker lifecycle phase"
        );
        info!(
            "Execution {} completed: {}",
            execution_id,
            if result.timed_out {
                "timeout"
            } else if result.is_success() {
                "success"
            } else {
                "failed"
            }
        );

        let mut result = result;
        let mut conn = self.pool.acquire().await?;
        if attune_common::repositories::delegated_execution::DelegatedExecutionRepository::find(
            &mut conn,
            execution_id,
        )
        .await?
        .explicit
        {
            result.stdout.clear();
            result.stderr.clear();
            result.result = None;
            if result.error.is_some() {
                result.error = Some("Execution failed; inspect protected result".into());
            }
        }
        Ok(ExecutionOutcome {
            transition,
            result: Ok(result),
        })
    }

    /// No parameters were restored and no code was launched at this point.
    /// Persist only a fixed diagnostic: there is no usable contract for safely
    /// including arbitrary action/schema errors or earlier attempt output.
    async fn fail_before_preparation(
        &self,
        execution_id: i64,
        message: &'static str,
    ) -> Result<ExecutionTransition> {
        ExecutionRepository::transition_worker(
            &self.pool,
            execution_id,
            Some(self.worker_id.load(Ordering::Acquire)).filter(|id| *id > 0),
            ExecutionStatus::Failed,
            Some(serde_json::json!({"succeeded":false,"error":message})),
        )
        .await
    }

    /// Load action from database using execution data
    async fn load_action(&self, execution: &Execution) -> Result<Action> {
        debug!("Loading action: {}", execution.action_ref);
        if let Some(snapshot) = execution.executable_snapshot.as_ref() {
            return Ok(snapshot.executable.action.clone());
        }
        let started = std::time::Instant::now();

        if let Some(action_id) = execution.action {
            let key = Self::action_id_cache_key(action_id);
            if let Some(action) = self.action_cache_by_id.get(&key).await {
                debug!(
                    entity = "action",
                    operation = "load",
                    caller = "worker.execute",
                    cache_hit = true,
                    cache_key = "id",
                    latency_ms = started.elapsed().as_millis() as u64
                );
                return Ok(action);
            }
        }
        let ref_key = Self::action_ref_cache_key(&execution.action_ref);
        if let Some(action) = self.action_cache_by_ref.get(&ref_key).await {
            debug!(
                entity = "action",
                operation = "load",
                caller = "worker.execute",
                cache_hit = true,
                cache_key = "ref",
                latency_ms = started.elapsed().as_millis() as u64
            );
            return Ok(action);
        }

        // Try to load by action ID if available
        if let Some(action_id) = execution.action {
            if let Some(action) =
                ActionRepository::find_by_id_including_retired(&self.pool, action_id).await?
            {
                self.cache_action(&action).await;
                debug!(
                    entity = "action",
                    operation = "load",
                    caller = "worker.execute",
                    cache_hit = false,
                    db_key = "id",
                    latency_ms = started.elapsed().as_millis() as u64
                );
                return Ok(action);
            }
        }

        // Fallback: look up by the full qualified action ref directly
        if let Some(action) =
            ActionRepository::find_by_ref(&self.pool, &execution.action_ref).await?
        {
            self.cache_action(&action).await;
            debug!(
                entity = "action",
                operation = "load",
                caller = "worker.execute",
                cache_hit = false,
                db_key = "ref",
                latency_ms = started.elapsed().as_millis() as u64
            );
            return Ok(action);
        }

        Err(Error::not_found(
            "Action",
            "ref",
            execution.action_ref.clone(),
        ))
    }

    fn action_id_cache_key(action_id: i64) -> String {
        action_id.to_string()
    }

    fn action_ref_cache_key(action_ref: &str) -> String {
        action_ref.to_string()
    }

    async fn cache_action(&self, action: &Action) {
        let id_key = Self::action_id_cache_key(action.id);
        let ref_key = Self::action_ref_cache_key(&action.r#ref);
        self.action_cache_by_id.insert(id_key, action.clone()).await;
        self.action_cache_by_ref
            .insert(ref_key, action.clone())
            .await;
    }

    fn runtime_id_cache_key(runtime_id: i64) -> String {
        runtime_id.to_string()
    }

    fn runtime_ref_cache_key(runtime_ref: &str) -> String {
        runtime_ref.to_string()
    }

    async fn cache_runtime(&self, runtime: &RuntimeModel) {
        let id_key = Self::runtime_id_cache_key(runtime.id);
        let ref_key = Self::runtime_ref_cache_key(&runtime.r#ref);
        self.runtime_cache_by_id
            .insert(id_key, runtime.clone())
            .await;
        self.runtime_cache_by_ref
            .insert(ref_key, runtime.clone())
            .await;
    }

    async fn load_runtime_by_id(&self, runtime_id: i64) -> Result<Option<RuntimeModel>> {
        let started = std::time::Instant::now();
        let key = Self::runtime_id_cache_key(runtime_id);
        if let Some(runtime) = self.runtime_cache_by_id.get(&key).await {
            debug!(
                entity = "runtime",
                operation = "load",
                caller = "worker.execute",
                cache_hit = true,
                cache_key = "id",
                latency_ms = started.elapsed().as_millis() as u64
            );
            return Ok(Some(runtime));
        }

        let query = format!(
            "SELECT {} FROM runtime WHERE id = $1",
            RUNTIME_SELECT_COLUMNS
        );
        let runtime = sqlx::query_as::<_, RuntimeModel>(&query)
            .bind(runtime_id)
            .fetch_optional(&self.pool)
            .await?;

        if let Some(runtime) = &runtime {
            self.cache_runtime(runtime).await;
        }
        debug!(
            entity = "runtime",
            operation = "load",
            caller = "worker.execute",
            cache_hit = false,
            db_key = "id",
            found = runtime.is_some(),
            latency_ms = started.elapsed().as_millis() as u64
        );

        Ok(runtime)
    }

    async fn load_runtime_versions(&self, runtime_id: i64) -> Result<Vec<RuntimeVersionModel>> {
        let started = std::time::Instant::now();
        let key = Self::runtime_id_cache_key(runtime_id);
        if let Some(versions) = self.runtime_versions_cache_by_runtime_id.get(&key).await {
            debug!(
                entity = "runtime_version",
                operation = "list_by_runtime",
                caller = "worker.execute",
                cache_hit = true,
                runtime_id,
                count = versions.len(),
                latency_ms = started.elapsed().as_millis() as u64
            );
            return Ok(versions);
        }

        let versions = RuntimeVersionRepository::find_by_runtime(&self.pool, runtime_id).await?;
        self.runtime_versions_cache_by_runtime_id
            .insert(key, versions.clone())
            .await;
        debug!(
            entity = "runtime_version",
            operation = "list_by_runtime",
            caller = "worker.execute",
            cache_hit = false,
            runtime_id,
            count = versions.len(),
            latency_ms = started.elapsed().as_millis() as u64
        );
        Ok(versions)
    }

    fn effective_action_log_settings(&self, action: &Action) -> LogAllocationSettings {
        LogAllocationSettings {
            policy: action
                .log_retention_policy
                .unwrap_or(self.execution_log_retention_policy),
            limit: action
                .log_retention_limit
                .unwrap_or(self.execution_log_retention_limit),
            segment_bytes: self.log_segment_config.max_segment_bytes as u64,
            flush_interval_ms: self.log_segment_config.flush_interval_ms,
        }
    }

    async fn prepare_execution_context(
        &self,
        execution: &Execution,
        action: &Action,
        preparation: &mut ExecutionPreparation,
        isolated_attempt: Option<uuid::Uuid>,
    ) -> Result<ExecutionContext> {
        debug!(
            "Preparing execution context for execution: {}",
            execution.id
        );

        let mut authority_connection = self.pool.acquire().await?;
        let authority = attune_common::repositories::execution_runtime_admission::ExecutionAdmissionRepository::existing_authority(
            &mut authority_connection, execution.id,
        ).await?;
        drop(authority_connection);
        if authority == attune_common::repositories::execution_runtime_admission::ExistingExecutionAuthority::Recorded {
            self.recorded_readiness.require_ready()?;
        }

        // Extract parameters from execution config.
        // Config is always stored in flat format: the config object itself
        // is the parameters map (e.g. {"url": "...", "method": "GET"}).
        let mut parameters = HashMap::new();

        let selected_config = execution
            .config
            .clone()
            .or_else(|| isolated_attempt.map(|_| serde_json::json!({})));
        let restored_config = if let Some(config) = selected_config {
            let (config, paths, snapshot) = self
                .secret_manager
                .restore_execution_parameters_with_snapshot(execution.id, config)
                .await?;
            preparation.parameter_secret_paths = Some(paths);
            if isolated_attempt.is_some() {
                preparation.input_snapshot = Some(snapshot);
            }
            Some(config)
        } else {
            None
        };
        let binding = &preparation.binding;
        let input_contract = attune_common::value_schema::CompiledContract::new(&binding.input)
            .map_err(|error| {
                Error::Validation(format!("Invalid stored input contract: {error}"))
            })?;
        input_contract
            .validate(restored_config.as_ref().unwrap_or(&serde_json::json!({})))
            .map_err(|error| {
                Error::Validation(format!("Bound input validation failed: {error}"))
            })?;

        if let Some(config) = &restored_config {
            if let JsonValue::Object(map) = config {
                debug!(
                    "Execution {} config restored (parameter_count: {})",
                    execution.id,
                    map.len(),
                );
                for (key, value) in map {
                    parameters.insert(key.clone(), value.clone());
                }
            } else {
                info!("Config is not an Object, cannot extract parameters");
            }
        } else {
            debug!("No execution config present");
        }

        debug!("Extracted {} parameters", parameters.len());

        // Prepare standard environment variables
        let mut env = HashMap::new();
        // Resolve the effective execution timeout from the snapshotted
        // `execution.timeout_seconds`, falling back defensively for legacy rows.
        let execution_timeout = resolve_execution_timeout_seconds(execution, action);

        // Standard execution context variables (see docs/QUICKREF-execution-environment.md)
        env.insert("ATTUNE_EXEC_ID".to_string(), execution.id.to_string());
        env.insert("ATTUNE_ACTION".to_string(), execution.action_ref.clone());
        env.insert("ATTUNE_API_URL".to_string(), self.api_url.clone());
        env.insert(
            "ATTUNE_ARTIFACTS_DIR".to_string(),
            if isolated_attempt.is_some() {
                attune_common::workload_artifact::ARTIFACTS_ROOT.into()
            } else {
                self.artifacts_dir.to_string_lossy().to_string()
            },
        );
        env.insert(
            "ATTUNE_RUNTIME_ENVS_DIR".to_string(),
            if isolated_attempt.is_some() {
                attune_common::workload_launch::RUNTIME_ROOT.into()
            } else {
                self.runtime_envs_dir.to_string_lossy().to_string()
            },
        );
        env.insert("ATTUNE_PACK_REF".to_string(), action.pack_ref.clone());
        if let Some(digest) = execution.pack_release_digest.as_ref() {
            env.insert("ATTUNE_PACK_RELEASE_DIGEST".to_string(), digest.clone());
        }
        if let Some(trace_tag) = execution.trace_tag.as_ref() {
            env.insert("ATTUNE_TRACE_TAG".to_string(), trace_tag.clone());
        }

        // Generate execution-scoped API token.
        //
        // Recorded admissions use the system sponsor, never the attributed
        // requester. Explicit delegated selections retain their active human owner.
        // There is no fallback from an admitted execution to mutable human IAM.
        if execution.permission_set_refs.is_empty() {
            debug!(
                "Execution {} has no permission sets; omitting ATTUNE_API_TOKEN",
                execution.id
            );
        } else {
            let mut admission_tx = self.pool.begin().await?;
            let persisted = ExecutionRepository::find_by_id(&mut *admission_tx, execution.id)
                .await?
                .filter(|current| {
                    current.status == ExecutionStatus::Running
                        && current.action_ref == execution.action_ref
                        && current.permission_set_refs == execution.permission_set_refs
                })
                .ok_or_else(|| {
                    Error::PermissionDenied(
                        "Execution credential source is missing, terminal or mismatched".into(),
                    )
                })?;
            let recorded = attune_common::repositories::execution_runtime_admission::ExecutionAdmissionRepository::existing_authority(
                &mut admission_tx, execution.id,
            ).await? == attune_common::repositories::execution_runtime_admission::ExistingExecutionAuthority::Recorded;
            admission_tx.commit().await?;
            if recorded {
                let token = crate::recorded_credentials::mint_recorded_execution_token(
                    &self.pool,
                    execution,
                    self.worker_id.load(Ordering::SeqCst),
                    isolated_attempt.unwrap_or_else(uuid::Uuid::new_v4),
                    execution_timeout as i64,
                    &self.jwt_config,
                    self.recorded_readiness,
                )
                .await?
                .ok_or_else(|| {
                    Error::PermissionDenied(
                        "Required recorded API credential was not issued".into(),
                    )
                })?;
                env.insert("ATTUNE_API_TOKEN".to_string(), token);
            } else {
                attune_common::repositories::delegated_execution::DelegatedExecutionRepository::validate(&mut *self.pool.acquire().await?,execution.id).await?;
                {
                    let attempt = attune_common::repositories::delegated_execution::DelegatedExecutionRepository::current_attempt(&mut *self.pool.acquire().await?,execution.id).await?;
                    let standard = self.standard_access_action_refs(execution).await;
                    let token = attune_common::auth::jwt::generate_delegated_execution_token(
                        resolve_execution_identity(persisted.executor, execution.id)?,
                        execution.id,
                        &execution.action_ref,
                        &self.jwt_config,
                        execution_timeout as i64,
                        &execution.permission_set_refs,
                        &standard,
                        attempt,
                    )
                    .map_err(|_| {
                        Error::PermissionDenied("Delegated credential issuance failed".into())
                    })?;
                    env.insert("ATTUNE_API_TOKEN".into(), token);
                }
            }
        }

        // Add rule and trigger context if execution was triggered by enforcement
        if let Some(enforcement_id) = execution.enforcement {
            if let Some(enforcement) =
                attune_common::repositories::event::EnforcementRepository::find_by_id(
                    &self.pool,
                    enforcement_id,
                )
                .await?
            {
                env.insert("ATTUNE_RULE".to_string(), enforcement.rule_ref);
                env.insert("ATTUNE_TRIGGER".to_string(), enforcement.trigger_ref);
            }
        }

        // The execution-only protocol delivers ALL parameters through stdin.
        // Keep the old local-worker context projection out of CP invocations.
        if isolated_attempt.is_none() {
            if let Some(config) = &restored_config {
                if let Some(JsonValue::Object(map)) = config.get("context") {
                    for (key, value) in map {
                        let env_key = format!("ATTUNE_CONTEXT_{}", key.to_uppercase());
                        let env_value = match value {
                            JsonValue::String(s) => s.clone(),
                            JsonValue::Number(n) => n.to_string(),
                            JsonValue::Bool(b) => b.to_string(),
                            _ => serde_json::to_string(value)?,
                        };
                        env.insert(env_key, env_value);
                    }
                }
            }
        }

        attune_common::execution_env::validate_snapshot_environment(
            execution.executable_snapshot.as_ref(),
            execution.env_vars.as_ref(),
        )?;
        let execution_env = if let Some(env_vars) = &execution.env_vars {
            serde_json::from_value::<HashMap<String, String>>(env_vars.clone()).map_err(|_| {
                Error::Validation(
                    "Execution env_vars must be an object with string values".to_string(),
                )
            })?
        } else {
            HashMap::new()
        };

        // Explicit, authorized secret inputs are restored into parameters.
        // No keystore owner namespace is enumerated into action stdin.
        let secrets = HashMap::new();

        // Determine entry point from action
        let entry_point = action.entrypoint.clone().unwrap_or_default();

        // Effective execution timeout resolved from execution.timeout_seconds
        // (snapshotted at creation) with defensive fallbacks for legacy rows.
        let timeout = Some(execution_timeout);

        // Load runtime information if specified
        let runtime_record = if let Some(snapshot) = execution.executable_snapshot.as_ref() {
            snapshot.executable.runtime.clone()
        } else if let Some(runtime_id) = action.runtime {
            match self.load_runtime_by_id(runtime_id).await {
                Ok(Some(runtime)) => {
                    debug!(
                        "Loaded runtime '{}' (ref: {}) for action '{}'",
                        runtime.name, runtime.r#ref, action.r#ref
                    );
                    Some(runtime)
                }
                Ok(None) => {
                    warn!(
                        "Runtime ID {} not found for action '{}'",
                        runtime_id, action.r#ref
                    );
                    None
                }
                Err(e) => {
                    warn!(
                        "Failed to load runtime {} for action '{}': {}",
                        runtime_id, action.r#ref, e
                    );
                    None
                }
            }
        } else {
            None
        };

        let runtime_name = runtime_record.as_ref().map(|runtime| {
            if action.launch.is_some() {
                normalize_runtime_name(&runtime.name)
            } else {
                // File runtimes are registered under their lowercased DB labels.
                runtime.name.to_lowercase()
            }
        });

        // --- Runtime Version Resolution ---
        // If the action declares a runtime_version_constraint (e.g., ">=3.12"),
        // query all registered versions for this runtime and select the best
        // match. The selected version's execution_config overrides the parent
        // runtime's config so the ProcessRuntime uses a version-specific
        // interpreter binary, environment commands, etc.
        let (runtime_config_override, runtime_env_dir_suffix, selected_runtime_version) =
            if isolated_attempt.is_some() {
                let snapshot = execution.executable_snapshot.as_ref().ok_or_else(|| {
                    Error::PermissionDenied("Pinned control runtime unavailable".into())
                })?;
                match runtime_record.as_ref() {
                    Some(runtime) => {
                        let versions = &snapshot.executable.runtime_versions;
                        match select_best_version(
                            versions,
                            action.runtime_version_constraint.as_deref(),
                        ) {
                            Some(version) => (
                                Some(serde_json::from_value::<RuntimeExecutionConfig>(
                                    version.execution_config.clone(),
                                )?),
                                Some(format!(
                                    "{}-{}",
                                    normalize_runtime_name(&runtime.name),
                                    version.version
                                )),
                                Some(version.version.clone()),
                            ),
                            None if versions.is_empty()
                                && action.runtime_version_constraint.is_none() =>
                            {
                                (
                                    Some(serde_json::from_value::<RuntimeExecutionConfig>(
                                        runtime.execution_config.clone(),
                                    )?),
                                    None,
                                    None,
                                )
                            }
                            None => {
                                return Err(Error::PermissionDenied(
                                    "No pinned runtime version satisfies the control execution"
                                        .into(),
                                ))
                            }
                        }
                    }
                    None if action.runtime.is_none()
                        && action.runtime_version_constraint.is_none() =>
                    {
                        (None, None, None)
                    }
                    None => {
                        return Err(Error::PermissionDenied(
                            "Pinned control runtime unavailable".into(),
                        ))
                    }
                }
            } else {
                self.resolve_runtime_version(&runtime_record, execution, action)
                    .await
            };
        // Parent configuration is captured too, even without registered versions.
        // Never substitute this worker's later authoring copy of the runtime.
        if let Some(runtime) = &runtime_record {
            attune_common::execution_env::validate_runtime_environment(&runtime.execution_config)?;
            if runtime.execution_config.get("lifecycle").is_some()
                && action.runtime_version_constraint.is_some()
                && selected_runtime_version.is_none()
            {
                return Err(Error::validation(
                    "No verified toolchain version satisfies the captured action constraint",
                ));
            }
        }
        let runtime_config_override = runtime_config_override.or_else(|| {
            runtime_record
                .as_ref()
                .map(|runtime| runtime.parsed_execution_config())
        });

        // Recheck the exact captured config selected after worker placement.
        attune_common::execution_env::validate_selected_environment(
            execution.executable_snapshot.as_ref(),
            selected_runtime_version.as_deref(),
            execution.env_vars.as_ref(),
        )?;

        // Determine the pack directory for this action
        let pack_dir = if isolated_attempt.is_some() {
            PathBuf::from(attune_common::workload_launch::PACK_ROOT)
        } else {
            execution
                .executable_snapshot
                .as_ref()
                .and_then(|snapshot| {
                    attune_common::pack_transport::release_cache_path(
                        &self.packs_base_dir,
                        &action.pack_ref,
                        &snapshot.release.digest,
                    )
                    .ok()
                })
                .unwrap_or_else(|| self.packs_base_dir.join(&action.pack_ref))
        };

        // Construct code_path for pack actions
        // Pack actions have their script files in packs/{pack_ref}/actions/{entrypoint}
        let code_path = if action.launch.is_some() {
            None
        } else if isolated_attempt.is_some() && !action.is_adhoc {
            let entry =
                attune_common::artifact_transport::ValidatedRelativePath::new(&entry_point)?;
            Some(pack_dir.join("actions").join(entry.as_path()))
        } else if isolated_attempt.is_some() {
            None
        } else if !action.is_adhoc {
            // This is a pack action, construct the file path
            let action_file_path = pack_dir.join("actions").join(&entry_point);

            if action_file_path.exists() {
                Some(action_file_path)
            } else {
                // Detailed diagnostics to help track down missing action files
                let pack_dir_exists = pack_dir.exists();
                let actions_dir = pack_dir.join("actions");
                let actions_dir_exists = actions_dir.exists();
                let actions_dir_contents: Vec<String> = if actions_dir_exists {
                    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Diagnostic directory listing is confined to the action pack directory derived from pack_ref.
                    std::fs::read_dir(&actions_dir)
                        .map(|entries| {
                            entries
                                .filter_map(|e| e.ok())
                                .map(|e| e.file_name().to_string_lossy().to_string())
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                warn!(
                    "Action file not found for action '{}': \
                     expected_path={}, \
                     packs_base_dir={}, \
                     pack_ref={}, \
                     entrypoint={}, \
                     pack_dir_exists={}, \
                     actions_dir_exists={}, \
                     actions_dir_contents={:?}",
                    action.r#ref,
                    action_file_path.display(),
                    self.packs_base_dir.display(),
                    action.pack_ref,
                    entry_point,
                    pack_dir_exists,
                    actions_dir_exists,
                    actions_dir_contents,
                );
                None
            }
        } else {
            None // Ad-hoc actions don't have files
        };

        // For shell actions without a file, use the entrypoint as inline code
        let code = inline_action_source(
            &entry_point,
            action.is_adhoc,
            runtime_name.as_deref(),
            runtime_config_override.as_ref(),
            code_path.is_some(),
        );

        // Resolve the working directory from the runtime's execution_config.
        // The ProcessRuntime also does this internally, but setting it in the
        // context allows the executor to override if needed.
        let working_dir: Option<StdPathBuf> = if isolated_attempt.is_some() {
            Some(pack_dir)
        } else if pack_dir.exists() {
            Some(if action.launch.is_some() {
                tokio::fs::canonicalize(pack_dir).await.map_err(|_| {
                    Error::Validation("action.launch.program_unavailable: Pinned release directory is unavailable".into())
                })?
            } else {
                pack_dir
            })
        } else {
            None
        };
        // CP planning uses pinned image/release metadata. Never inspect or
        // resolve programs against the control-plane machine's PATH or ARCH.
        let native_invocation = if isolated_attempt.is_some() {
            None
        } else if let Some(launch) = &action.launch {
            let snapshot = execution.executable_snapshot.as_ref().ok_or_else(|| {
                Error::Validation("Command launch requires an admitted executable snapshot".into())
            })?;
            launch.validate_action_source(
                action.entrypoint.as_deref(),
                action.parameter_delivery,
                action.parameter_format,
            )?;
            if runtime_name.as_deref() != Some("native") {
                return Err(Error::Validation(
                    "Command launch requires a native runtime".into(),
                ));
            }
            let root = working_dir.clone().ok_or_else(|| {
                Error::Validation(
                    "action.launch.program_unavailable: Pinned release directory is unavailable"
                        .into(),
                )
            })?;
            let release =
                attune_common::repositories::pack_release::PackReleaseRepository::find_by_id(
                    &self.pool,
                    snapshot.release.id,
                )
                .await?
                .filter(|release| {
                    release.digest == snapshot.release.digest && release.pack_ref == action.pack_ref
                })
                .ok_or_else(|| {
                    Error::Validation(
                        "action.launch.program_unavailable: Pinned command release is unavailable"
                            .into(),
                    )
                })?;
            let manifest = if matches!(
                launch,
                attune_common::pack_format::LaunchSpec::Native { .. }
            ) {
                Some(
                    serde_json::from_value::<attune_common::pack_format::Manifest>(
                        release.manifest,
                    )
                    .map_err(|_| Error::Validation("Invalid pinned release manifest".into()))?,
                )
            } else {
                None
            };
            let launch_copy = launch.clone();
            let reference = action.r#ref.clone();
            let environment = self.child_environment.clone();
            let selected = tokio::task::spawn_blocking(move || {
                attune_common::action_cli::select_program(
                    &launch_copy,
                    &root,
                    manifest.as_ref(),
                    &reference,
                    &environment,
                )
            })
            .await
            .map_err(|_| Error::Validation("Program selection task failed".into()))??;
            let empty = serde_json::json!({});
            let plan = attune_common::action_cli::ArgumentPlan::from_bound_contract(
                action.param_schema.as_ref().unwrap_or(&empty),
                launch.action_arguments()?,
                binding.input.clone(),
            )
            .map_err(|error| Error::Validation(error.to_string()))?;
            let schema = action.param_schema.as_ref().unwrap_or(&empty);
            let cli_enabled = matches!(
                launch,
                attune_common::pack_format::LaunchSpec::Command { .. }
            ) || !launch.action_arguments()?.is_empty()
                || attune_common::action_cli::schema_for_binding(schema)
                    .map_err(|error| Error::Validation(error.to_string()))?
                    != *schema
                || action.parameter_delivery == attune_common::models::ParameterDelivery::None;
            let input = if cli_enabled {
                let delivery = match action.parameter_delivery {
                    attune_common::models::ParameterDelivery::None => {
                        attune_common::action_cli::InputDelivery::None
                    }
                    attune_common::models::ParameterDelivery::Stdin => {
                        attune_common::action_cli::InputDelivery::Stdin
                    }
                    _ => {
                        return Err(Error::Validation(
                            "Unsupported command input delivery".into(),
                        ))
                    }
                };
                let input = plan
                    .materialize(
                        selected.argument_budget_program()?,
                        &serde_json::to_value(&parameters)?,
                        preparation.parameter_secret_paths.as_deref().unwrap_or(&[]),
                        delivery,
                        attune_common::action_cli::ArgumentLimits::default(),
                    )
                    .map_err(|error| Error::Validation(error.to_string()))?;
                Some(input)
            } else {
                None
            };
            attune_common::repositories::execution_program::ExecutionProgramRepository::record(
                &self.pool,
                execution.id,
                self.worker_id.load(Ordering::Acquire),
                snapshot.release.id,
                &selected,
            )
            .await?;
            Some(crate::runtime::NativeInvocation { selected, input })
        } else {
            None
        };
        let logs = self
            .allocate_execution_log_artifacts(execution, action, preparation)
            .await?;
        let (stdout_log_writer, stderr_log_writer) =
            (Some(logs.stdout_writer), Some(logs.stderr_writer));

        let context = ExecutionContext {
            native_invocation,
            execution_id: execution.id,
            action_ref: execution.action_ref.clone(),
            parameters,
            env,
            execution_env,
            secrets, // Passed securely via stdin
            timeout,
            working_dir,
            entry_point,
            code,
            code_path,
            runtime_name,
            runtime_config_override,
            runtime_env_dir_suffix,
            selected_runtime_version,
            max_stdout_bytes: self.max_stdout_bytes,
            max_stderr_bytes: self.max_stderr_bytes,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer,
            stderr_log_writer,
            parameter_delivery: action.parameter_delivery,
            parameter_format: action.parameter_format,
            output_format: binding.output_format,
            output_policy: binding.output_policy,
            out_schema: binding.output.clone(),
            cancel_token: None,
        };

        Ok(context)
    }

    async fn standard_access_action_refs(&self, execution: &Execution) -> Vec<String> {
        let mut refs = vec![execution.action_ref.clone()];

        if execution.workflow_task.is_some() {
            if let Some(parent_id) = execution.parent {
                match ExecutionRepository::find_by_id(&self.pool, parent_id).await {
                    Ok(Some(parent)) => refs.push(parent.action_ref),
                    Ok(None) => warn!(
                        "Execution {} references missing workflow parent {}; standard token access will not include workflow action scope",
                        execution.id, parent_id
                    ),
                    Err(e) => warn!(
                        "Failed to load workflow parent {} for execution {}; standard token access will not include workflow action scope: {}",
                        parent_id, execution.id, e
                    ),
                }
            }
        }

        refs.sort();
        refs.dedup();
        refs
    }

    /// Resolve the best runtime version for an action, if applicable.
    ///
    /// Returns a tuple of:
    /// - Optional `RuntimeExecutionConfig` override (from the selected version)
    /// - Optional env dir suffix (e.g., `"python-3.12"`) for per-version isolation
    /// - Optional version string for logging (e.g., `"3.12"`)
    ///
    /// If the action has no `runtime_version_constraint`, or no versions are
    /// registered for its runtime, all three are `None` and the parent runtime's
    /// config is used as-is.
    async fn resolve_runtime_version(
        &self,
        runtime_record: &Option<RuntimeModel>,
        execution: &Execution,
        action: &Action,
    ) -> (
        Option<RuntimeExecutionConfig>,
        Option<String>,
        Option<String>,
    ) {
        let runtime = match runtime_record {
            Some(r) => r,
            None => return (None, None, None),
        };

        // Query all versions for this runtime
        let versions = match execution.executable_snapshot.as_ref() {
            Some(snapshot) => snapshot.executable.runtime_versions.clone(),
            None => match self.load_runtime_versions(runtime.id).await {
                Ok(v) if !v.is_empty() => v,
                Ok(_) => {
                    // No versions registered — use parent runtime config as-is
                    if action.runtime_version_constraint.is_some() {
                        warn!(
                        "Action '{}' declares runtime_version_constraint '{}' but runtime '{}' \
                         has no registered versions. Using parent runtime config.",
                        action.r#ref,
                        action.runtime_version_constraint.as_deref().unwrap_or(""),
                        runtime.name,
                    );
                    }
                    return (None, None, None);
                }
                Err(e) => {
                    warn!(
                        "Failed to load runtime versions for runtime '{}' (id {}): {}. \
                     Using parent runtime config.",
                        runtime.name, runtime.id, e,
                    );
                    return (None, None, None);
                }
            },
        };

        let versions = versions
            .into_iter()
            .filter(|version| {
                attune_common::runtime_lifecycle::validate_version_mode(
                    &runtime.execution_config,
                    &version.execution_config,
                )
                .is_ok()
            })
            .collect();

        let constraint = action.runtime_version_constraint.as_deref();
        let local_versions = self
            .filter_versions_for_worker(runtime, execution.worker, versions, action)
            .await;

        match select_best_version(&local_versions, constraint) {
            Some(selected) => {
                let version_config = selected.parsed_execution_config();
                let rt_name = runtime.name.to_lowercase();
                let env_suffix = format!("{}-{}", rt_name, selected.version);

                info!(
                    "Selected runtime version '{}' (id {}) for action '{}' \
                     (constraint: {}, runtime: '{}'). Env dir suffix: '{}'",
                    selected.version,
                    selected.id,
                    action.r#ref,
                    constraint.unwrap_or("none"),
                    runtime.name,
                    env_suffix,
                );

                (
                    Some(version_config),
                    Some(env_suffix),
                    Some(selected.version.clone()),
                )
            }
            None => {
                if let Some(c) = constraint {
                    warn!(
                        "No locally available runtime version matches constraint '{}' for action '{}' \
                         on worker {:?} (runtime: '{}'). Using parent runtime config as fallback.",
                        c, action.r#ref, execution.worker, runtime.name,
                    );
                } else {
                    debug!(
                        "No default or available version found for runtime '{}'. \
                         Using parent runtime config.",
                        runtime.name,
                    );
                }
                (None, None, None)
            }
        }
    }

    async fn filter_versions_for_worker(
        &self,
        runtime: &RuntimeModel,
        worker_id: Option<i64>,
        versions: Vec<attune_common::models::RuntimeVersion>,
        action: &Action,
    ) -> Vec<attune_common::models::RuntimeVersion> {
        let Some(worker_id) = worker_id else {
            warn!(
                "Execution for action '{}' has no assigned worker while resolving runtime versions for '{}'; using base runtime fallback",
                action.r#ref,
                runtime.name,
            );
            return Vec::new();
        };

        let worker = match WorkerRepository::find_by_id(&self.pool, worker_id).await {
            Ok(Some(worker)) => worker,
            Ok(None) => {
                warn!(
                    "Assigned worker {} not found while resolving runtime versions for action '{}'; using base runtime fallback",
                    worker_id,
                    action.r#ref,
                );
                return Vec::new();
            }
            Err(e) => {
                warn!(
                    "Failed to load worker {} while resolving runtime versions for action '{}': {}. Using base runtime fallback.",
                    worker_id,
                    action.r#ref,
                    e,
                );
                return Vec::new();
            }
        };

        let advertised_versions = Self::worker_runtime_versions_for_runtime(&worker, runtime);
        if advertised_versions.is_empty() {
            warn!(
                "Worker {} does not advertise local runtime versions for '{}' while resolving action '{}'; using base runtime fallback",
                worker.name,
                runtime.name,
                action.r#ref,
            );
            return Vec::new();
        }

        versions
            .into_iter()
            .filter(|version| Self::version_matches_worker(version, &advertised_versions))
            .map(|mut version| {
                if runtime.execution_config.get("lifecycle").is_some() {
                    version.available = true;
                }
                version
            })
            .collect()
    }

    fn worker_runtime_versions_for_runtime(worker: &Worker, runtime: &RuntimeModel) -> Vec<String> {
        if runtime.execution_config.get("lifecycle").is_some() {
            return attune_common::runtime_lifecycle::available_versions(
                worker.capabilities.as_ref(),
                runtime,
            );
        }
        let mut versions = Vec::new();
        let candidate_runtime_names: Vec<String> = runtime
            .aliases
            .iter()
            .map(|alias| normalize_runtime_name(alias))
            .chain(std::iter::once(normalize_runtime_name(&runtime.name)))
            .collect();

        let Some(capabilities) = worker
            .capabilities
            .as_ref()
            .and_then(|value| value.as_object())
        else {
            return versions;
        };

        if let Some(runtime_versions) = capabilities.get("runtime_versions") {
            if let Some(runtime_versions_obj) = runtime_versions.as_object() {
                for runtime_name in &candidate_runtime_names {
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
            if let Some(detected_interpreters) = capabilities.get("detected_interpreters") {
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

    fn version_matches_worker(
        version: &attune_common::models::RuntimeVersion,
        advertised_versions: &[String],
    ) -> bool {
        advertised_versions.iter().any(|advertised_version| {
            advertised_version == &version.version
                || matches_constraint(advertised_version, &version.version).unwrap_or(false)
        })
    }

    async fn prepare_native_process(
        &self,
        execution: &Execution,
    ) -> Result<attune_common::isolated_process::ProcessSpec> {
        use attune_common::{
            isolated_process::ProcessSpec,
            pack_registry::storage::PackReleaseManifest,
            repositories::{
                delegated_execution::DelegatedExecutionRepository,
                execution_runtime_admission::{
                    ExecutionAdmissionRepository, ExistingExecutionAuthority,
                },
                pack_release::PackReleaseRepository,
                runtime_authority::RuntimeAuthorityRepository,
            },
            workload_archive::ArchivePolicy,
            workload_launch::{
                LaunchInputKind, LaunchInvocation, LaunchPurpose, LaunchRoot, LaunchTarget,
                PackAccess, PinnedLaunchInput, Sha256Digest,
            },
            workload_pack_input::{release_zip, ReleaseInputPin},
        };
        let denied = || Error::PermissionDenied("Native action preparation unavailable".into());
        let native = self.native_preparation.as_ref().ok_or_else(denied)?;
        let worker = self.worker_id.load(Ordering::Acquire);
        let incarnation = *self.process_incarnation.get().ok_or_else(denied)?;
        if worker <= 0
            || execution.worker != Some(worker)
            || execution.status != ExecutionStatus::Running
        {
            return Err(denied());
        }
        let snapshot = execution.executable_snapshot.as_ref().ok_or_else(denied)?;
        if execution.pack_release != Some(snapshot.release.id)
            || execution.pack_release_digest.as_deref() != Some(snapshot.release.digest.as_str())
        {
            return Err(denied());
        }
        let release = PackReleaseRepository::find_by_id(&self.pool, snapshot.release.id)
            .await?
            .filter(|release| {
                release.digest == snapshot.release.digest
                    && release.pack_ref == snapshot.executable.action.pack_ref
            })
            .ok_or_else(denied)?;
        let manifest: PackReleaseManifest =
            serde_json::from_value(release.manifest).map_err(|_| denied())?;
        if manifest.version != release.version || manifest.pack_ref != release.pack_ref {
            return Err(denied());
        }
        let mut tx = self.pool.begin().await?;
        let attempt = match ExecutionAdmissionRepository::selected_authority(&mut tx, execution.id)
            .await?
        {
            ExistingExecutionAuthority::Delegated => {
                DelegatedExecutionRepository::current_attempt(&mut tx, execution.id).await?
            }
            ExistingExecutionAuthority::Recorded => {
                self.recorded_readiness.require_ready()?;
                RuntimeAuthorityRepository::current_worker_attempt(&mut tx, execution.id, worker)
                    .await?
            }
        };
        tx.commit().await?;
        let master = self.secret_manager.encryption_key().ok_or_else(denied)?;
        if let Some(retained) = attune_common::repositories::execution_process::ExecutionProcessRepository::retained_for_attempt(
            &self.pool, execution.id, attempt, master,
        ).await? {
            // A replaced incarnation is rejected by the existing target check.
            // Never mint another effect ID after an unknown create/commit.
            return Ok(retained);
        }
        // Existing pack transport materializes this primary immutable archive
        // for both volume and API managers. Never repack an unpacked tree.
        let archive = attune_common::pack_transport::release_cache_path(
            &self.packs_base_dir,
            &release.pack_ref,
            &release.digest,
        )?
        .parent()
        .ok_or_else(denied)?
        .join("pack.tar.gz");
        let source = ReleaseInputPin {
            pack_ref: release.pack_ref,
            size_bytes: u64::try_from(release.archive_size).map_err(|_| denied())?,
            sha256: Sha256Digest::parse(&release.digest)?,
        };
        let ceiling = native.config.storage;
        let runner = native.runner.clone();
        let cache = native.cache.clone();
        let release_id = release.id;
        let release_digest = release.digest;
        let pack = tokio::task::spawn_blocking(move || {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(archive)
                .map_err(|_| Error::validation("Pinned release archive unavailable"))?;
            let metadata = file
                .metadata()
                .map_err(|_| Error::validation("Pinned release archive unavailable"))?;
            if !metadata.is_file()
                || metadata.nlink() != 1
                || metadata.mode() & 0o022 != 0
                || !(metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() })
            {
                return Err(Error::PermissionDenied(
                    "Pinned release archive ownership invalid".into(),
                ));
            }
            let prepared = release_zip(
                file,
                &source,
                &manifest,
                attune_common::pack_format::Limits {
                    compressed_bytes: ceiling.compressed_bytes,
                    extracted_bytes: ceiling.expanded_bytes,
                    file_bytes: ceiling.expanded_bytes,
                    entries: ceiling.entries,
                    ..Default::default()
                },
                &ArchivePolicy {
                    root: LaunchInputKind::Pack,
                    max_compressed_bytes: ceiling.compressed_bytes,
                    max_expanded_bytes: ceiling.expanded_bytes,
                    max_files: ceiling.entries,
                },
            )?;
            let pin = PinnedLaunchInput {
                // Pin IDs distinguish slots inside this one plan. Object ID and
                // version come exclusively from the retained release record;
                // the public Runner's operator pin is preserved unchanged.
                pin_id: if runner.pin_id == 1 { 2 } else { 1 },
                kind: LaunchInputKind::Pack,
                object_id: release_id,
                object_version: release_digest,
                size_bytes: prepared.size_bytes(),
                sha256: prepared.sha256().clone(),
                max_expanded_bytes: prepared.expanded_bytes(),
                max_files: prepared.entries(),
            };
            let mut body = prepared.into_file();
            cache
                .install_input(&pin, body.as_file_mut())
                .map_err(|_| Error::validation("Pinned release ZIP installation failed"))?;
            Ok::<_, Error>(pin)
        })
        .await
        .map_err(|_| Error::invalid_state("Native release preparation did not complete"))??;
        Ok(ProcessSpec {
            intent: uuid::Uuid::new_v4(),
            purpose: LaunchPurpose::Action,
            target: LaunchTarget::Execution {
                execution: execution.id,
                worker,
                worker_instance: incarnation,
                attempt,
            },
            image: native.config.image.clone(),
            platform: native.config.platform,
            inputs: vec![pack, native.runner.clone()],
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: None,
            // The existing context preparation below selects and freezes the
            // executable, static argv0 and selected env before any provider IO.
            invocation: LaunchInvocation {
                argv: Vec::new(),
                env: Default::default(),
                working_root: LaunchRoot::Pack,
            },
        })
    }

    async fn isolated_execution_owner(&self, execution: i64) -> Result<i64> {
        use attune_common::repositories::{
            delegated_execution::DelegatedExecutionRepository,
            execution_runtime_admission::{
                ExecutionAdmissionRepository, ExistingExecutionAuthority,
            },
        };
        let mut tx = self.pool.begin().await?;
        let owner =
            match ExecutionAdmissionRepository::selected_authority(&mut tx, execution).await? {
                ExistingExecutionAuthority::Delegated => {
                    DelegatedExecutionRepository::validate(&mut tx, execution)
                        .await?
                        .owner
                        .ok_or_else(|| {
                            Error::PermissionDenied("Isolated action owner unavailable".into())
                        })?
                }
                ExistingExecutionAuthority::Recorded => {
                    let (record, fence) = ExecutionAdmissionRepository::require_with_fence(
                        &mut tx,
                        attune_common::repositories::runtime_authority::RuntimeOwner::Execution(
                            execution,
                        ),
                    )
                    .await?;
                    fence.ensure_fresh_on(&mut tx).await?;
                    record.authority.owner.sponsor_identity_id
                }
            };
        tx.commit().await?;
        Ok(owner)
    }

    /// A prerequisite effect owned by this action's existing lifecycle. It has
    /// no action input/API credential, log custody or completion capability.
    async fn inspect_isolated_command(
        &self,
        context: &ExecutionContext,
        preparation: &ExecutionPreparation,
        action: &attune_common::isolated_process::ProcessSpec,
        program: &str,
    ) -> Result<attune_common::action_cli::SelectedProgram> {
        use attune_common::{
            repositories::{
                execution_input::SelectedExecutionInput,
                execution_process::ExecutionProcessRepository as Effects,
            },
            workload_launch::{
                LaunchPurpose, LaunchRoot, LaunchStopReason, LaunchTarget, RUNNER_ROOT,
            },
        };
        let unavailable =
            || Error::PermissionDenied("Isolated command inspection unavailable".into());
        let master = self
            .secret_manager
            .encryption_key()
            .ok_or_else(unavailable)?;
        if let Some(selected) = Effects::inspected_program(&self.pool, action, master).await? {
            let attune_common::workload_launch::LaunchTarget::Execution {
                execution, attempt, ..
            } = action.target
            else {
                return Err(unavailable());
            };
            let retained = Effects::retained_for_purpose(
                &self.pool,
                execution,
                attempt,
                LaunchPurpose::RuntimeProbe,
                master,
            )
            .await?
            .ok_or_else(unavailable)?;
            let resource = Effects::retained_resource(&self.pool, retained.intent)
                .await?
                .ok_or_else(unavailable)?;
            self.native_backend
                .as_ref()
                .ok_or_else(unavailable)?
                .remove_stopped(&retained, &resource)
                .await?;
            return Ok(selected);
        }
        let LaunchTarget::Execution {
            execution, attempt, ..
        } = action.target
        else {
            return Err(unavailable());
        };
        let probe = if let Some(retained) = Effects::retained_for_purpose(
            &self.pool,
            execution,
            attempt,
            LaunchPurpose::RuntimeProbe,
            master,
        )
        .await?
        {
            retained
        } else {
            let mut probe = action.clone();
            probe.intent = uuid::Uuid::new_v4();
            probe.purpose = LaunchPurpose::RuntimeProbe;
            probe.invocation.argv = vec![
                format!("{RUNNER_ROOT}/linux-{}", probe.platform.native_arch()),
                "inspect-command-program".into(),
                program.to_owned(),
            ];
            // Resolve only the captured native runtime's explicit guest PATH.
            // No manager PATH, action credential, parameters or image/service
            // environment enters the inspection process.
            let config = context
                .runtime_config_override
                .as_ref()
                .ok_or_else(unavailable)?;
            let environment = crate::runtime::launch_planning::action_environment(
                config,
                &Default::default(),
                program,
                false,
                probe.pack_access,
            )
            .map_err(|_| unavailable())?;
            let path = environment.get("PATH").cloned().ok_or_else(unavailable)?;
            probe.invocation.env = [("PATH".into(), path)].into();
            probe.invocation.working_root = LaunchRoot::Pack;
            probe
        };
        let selected = SelectedExecutionInput::new(
            serde_json::to_value(&context.parameters)?,
            preparation.input_snapshot.clone().ok_or_else(unavailable)?,
        )?;
        let owner = self.isolated_execution_owner(execution).await?;
        Effects::reserve(&self.pool, &probe, owner, &selected, master).await?;
        let backend = self.native_backend.as_ref().ok_or_else(unavailable)?;
        let launcher = backend.launcher();
        let create = Effects::begin_create(&self.pool, probe.intent, master).await?;
        let resource = launcher.prepare(&probe, create).await?;
        Effects::record_resource(&self.pool, probe.intent, &resource).await?;
        let mut process = launcher.start(&probe, &resource).await?;
        if process.resource != resource {
            process
                .control
                .stop(LaunchStopReason::LaunchFailure)
                .await?;
            return Err(unavailable());
        }
        process.control = Arc::new(RecordingProcessControl {
            inner: process.control,
            pool: self.pool.clone(),
            intent: probe.intent,
            resource: resource.clone(),
        });
        let payload =
            match Effects::consume_stdin(&self.pool, probe.intent, &resource, master).await {
                Ok(payload) => payload,
                Err(error) => {
                    process
                        .control
                        .stop(LaunchStopReason::SourceRevoked)
                        .await?;
                    return Err(error);
                }
            };
        let result = crate::runtime::process_executor::execute_isolated_cancellable(
            process,
            payload,
            crate::runtime::process_executor::IsolatedCaptureOptions {
                timeout_secs: context.timeout,
                max_stdout_bytes: 16 * 1024,
                max_stderr_bytes: 16 * 1024,
                output_format: attune_common::models::OutputFormat::Text,
                output_policy: Default::default(),
                out_schema: None,
                cancel_token: context.cancel_token.clone(),
                stdout_log_writer: None,
                stderr_log_writer: None,
            },
        )
        .await
        .map_err(|_| unavailable())?;
        let capture = async {
            if result.exit_code != 0
                || result.error.is_some()
                || result.timed_out
                || result.logs_incomplete
                || result.stdout_truncated
                || result.stderr_truncated
            {
                return Err(unavailable());
            }
            let selected = serde_json::from_str(&result.stdout).map_err(|_| unavailable())?;
            Effects::complete_command_inspection(
                &self.pool,
                probe.intent,
                &resource,
                &selected,
                master,
            )
            .await?;
            Ok(selected)
        }
        .await;
        // This read-only probe has no file/custody obligations. Join factual
        // cleanup even on rejected metadata/negative exit; uncertain removal
        // never becomes authority to create or inspect again.
        let cleanup = backend.remove_stopped(&probe, &resource).await;
        let selected = capture?;
        cleanup?;
        Ok(selected)
    }

    /// The existing trusted preparer selects the program and environment. A
    /// launcher supplies effects, never executable discovery or domain policy.
    /// Native file selection uses the immutable manifest + admitted platform;
    /// no control-plane PATH, architecture, unpacked tree or local probe enters it.
    async fn prepare_isolated_native_invocation(
        &self,
        context: &ExecutionContext,
        preparation: &ExecutionPreparation,
        spec: &mut attune_common::isolated_process::ProcessSpec,
    ) -> Result<()> {
        use attune_common::{
            pack_registry::storage::PackReleaseManifest,
            repositories::pack_release::PackReleaseRepository,
            workload_launch::{LaunchInputKind, LaunchRoot, PackAccess, PACK_ROOT},
        };
        let unavailable =
            || Error::PermissionDenied("Isolated native program proof unavailable".into());
        let execution = ExecutionRepository::find_by_id(&self.pool, context.execution_id)
            .await?
            .ok_or_else(unavailable)?;
        let snapshot = execution
            .executable_snapshot
            .as_ref()
            .ok_or_else(unavailable)?;
        let action = &snapshot.executable.action;
        // These paths remain explicitly unactivated until their actual pinned
        // setup/image/command producer is integrated. No host fallback.
        let config = context
            .runtime_config_override
            .as_ref()
            .ok_or_else(unavailable)?;
        if spec.purpose != attune_common::workload_launch::LaunchPurpose::Action
            || spec.inputs.len() != 2
            || !matches!(config.interpreter.binary.as_str(), "" | "native" | "none")
            || crate::runtime::launch_planning::needs_runtime(config)
            || spec.stage.is_some()
            || spec.pack_access != PackAccess::ReadOnly
        {
            return Err(unavailable());
        }
        let pack = spec
            .inputs
            .iter()
            .find(|pin| pin.kind == LaunchInputKind::Pack)
            .ok_or_else(unavailable)?;
        if pack.object_id != snapshot.release.id
            || pack.object_version != snapshot.release.digest
            || !spec
                .inputs
                .iter()
                .any(|pin| pin.kind == LaunchInputKind::Runner)
        {
            return Err(unavailable());
        }
        let release = PackReleaseRepository::find_by_id(&self.pool, snapshot.release.id)
            .await?
            .filter(|release| {
                release.digest == snapshot.release.digest && release.pack_ref == action.pack_ref
            })
            .ok_or_else(unavailable)?;
        let manifest: PackReleaseManifest =
            serde_json::from_value(release.manifest).map_err(|_| unavailable())?;
        if manifest.format_version != 1
            || manifest.pack_ref != action.pack_ref
            || manifest.version != release.version
        {
            return Err(unavailable());
        }
        let selected = if let Some(launch) = &action.launch {
            launch.validate_action_source(
                action.entrypoint.as_deref(),
                action.parameter_delivery,
                action.parameter_format,
            )?;
            let cache = self
                .native_preparation
                .as_ref()
                .ok_or_else(unavailable)?
                .cache
                .clone();
            let pin = pack.clone();
            let launch = launch.clone();
            let reference = action.r#ref.clone();
            let platform = spec.platform;
            if let attune_common::pack_format::LaunchSpec::Command { program, .. } = &launch {
                self.inspect_isolated_command(context, preparation, spec, program)
                    .await?
            } else {
                tokio::task::spawn_blocking(move || {
                    let body = cache.open_input_archive(&pin).map_err(|_| {
                        Error::PermissionDenied("Native metadata archive unavailable".into())
                    })?;
                    attune_common::action_cli::select_released_native(
                        &launch, &manifest, body, &reference, platform,
                    )
                })
                .await
                .map_err(|_| unavailable())??
            }
        } else {
            let relative = attune_common::artifact_transport::ValidatedRelativePath::new(
                &context.entry_point,
            )?;
            let file = format!("actions/{}", relative.as_str());
            let variant = format!("{file}/linux-{}", spec.platform.native_arch());
            let selected = manifest
                .files
                .iter()
                .find(|entry| entry.path == file && entry.executable)
                .or_else(|| {
                    manifest
                        .files
                        .iter()
                        .find(|entry| entry.path == variant && entry.executable)
                })
                .ok_or_else(unavailable)?;
            let program = format!("{PACK_ROOT}/{}", selected.path);
            attune_common::action_cli::SelectedProgram {
                path: PathBuf::from(&program),
                sha256: selected.sha256.clone(),
                invocation_name: program,
                artifact: None,
                variant: None,
            }
        };
        let arguments = if let Some(launch) = &action.launch {
            use attune_common::action_cli::{ArgumentLimits, ArgumentPlan, InputDelivery};
            let schema = action
                .param_schema
                .as_ref()
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let plan = ArgumentPlan::from_bound_contract(
                &schema,
                launch.action_arguments()?,
                preparation.binding.input.clone(),
            )
            .map_err(|_| unavailable())?;
            let delivery = match action.parameter_delivery {
                attune_common::models::ParameterDelivery::None => InputDelivery::None,
                attune_common::models::ParameterDelivery::Stdin => InputDelivery::Stdin,
                _ => return Err(unavailable()),
            };
            let input = plan
                .materialize(
                    selected.argument_budget_program()?,
                    &serde_json::to_value(&context.parameters)?,
                    preparation.parameter_secret_paths.as_deref().unwrap_or(&[]),
                    delivery,
                    ArgumentLimits::default(),
                )
                .map_err(|_| unavailable())?;
            input.argv().to_vec()
        } else {
            Vec::new()
        };
        spec.invocation.argv =
            crate::runtime::launch_planning::cli_exec_argv(spec.platform, &selected, &arguments)
                .map_err(|_| unavailable())?;
        let file = if matches!(
            action.launch,
            Some(attune_common::pack_format::LaunchSpec::Command { .. })
        ) {
            selected.path.to_str().ok_or_else(unavailable)?.to_owned()
        } else if action.launch.is_some() {
            selected
                .path
                .strip_prefix(PACK_ROOT)
                .map_err(|_| unavailable())?
                .to_str()
                .ok_or_else(unavailable)?
                .trim_start_matches('/')
                .to_owned()
        } else {
            format!("actions/{}", context.entry_point)
        };
        let mut environment = context
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        environment.extend(
            context
                .execution_env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        spec.invocation.env = crate::runtime::launch_planning::action_environment(
            config,
            &environment,
            &file,
            false,
            spec.pack_access,
        )
        .map_err(|_| unavailable())?;
        spec.invocation.working_root = LaunchRoot::Pack;
        spec.timeout_ms = context
            .timeout
            .map(|seconds| seconds.checked_mul(1000).ok_or_else(unavailable))
            .transpose()?;
        if !matches!(
            action.launch,
            Some(attune_common::pack_format::LaunchSpec::Command { .. })
        ) {
            attune_common::repositories::execution_program::ExecutionProgramRepository::record(
                &self.pool,
                execution.id,
                self.worker_id.load(Ordering::Acquire),
                snapshot.release.id,
                &selected,
            )
            .await?;
        }
        Ok(())
    }

    /// Execute the action using the runtime registry
    async fn execute_isolated_action(
        &self,
        context: ExecutionContext,
        preparation: &ExecutionPreparation,
        mut spec: attune_common::isolated_process::ProcessSpec,
        launcher: Arc<dyn attune_common::isolated_process::ProcessLauncher>,
    ) -> Result<ExecutionResult> {
        use attune_common::repositories::{
            execution_input::SelectedExecutionInput,
            execution_process::ExecutionProcessRepository as Effects,
        };
        let master = self.secret_manager.encryption_key().ok_or_else(|| {
            Error::PermissionDenied("Isolated action context encryption unavailable".into())
        })?;
        let retained = Effects::retained_for_attempt(
            &self.pool,
            context.execution_id,
            match spec.target {
                attune_common::workload_launch::LaunchTarget::Execution { attempt, .. } => attempt,
                _ => {
                    return Err(Error::PermissionDenied(
                        "Isolated action target unavailable".into(),
                    ))
                }
            },
            master,
        )
        .await?;
        if retained
            .as_ref()
            .is_none_or(|value| value.intent != spec.intent)
        {
            self.prepare_isolated_native_invocation(&context, preparation, &mut spec)
                .await?;
        }
        let selected = SelectedExecutionInput::new(
            serde_json::to_value(&context.parameters)?,
            preparation.input_snapshot.clone().ok_or_else(|| {
                Error::PermissionDenied("Isolated action input unavailable".into())
            })?,
        )?;
        let owner = self.isolated_execution_owner(context.execution_id).await?;
        Effects::reserve(&self.pool, &spec, owner, &selected, master).await?;
        let create = Effects::begin_create(&self.pool, spec.intent, master).await?;
        let resource = launcher.prepare(&spec, create).await?;
        Effects::record_resource(&self.pool, spec.intent, &resource).await?;
        let mut process = launcher.start(&spec, &resource).await?;
        if process.resource != resource {
            let _ = process
                .control
                .stop(attune_common::workload_launch::LaunchStopReason::LaunchFailure)
                .await;
            return Err(Error::PermissionDenied(
                "Isolated action resource changed".into(),
            ));
        }
        process.control = Arc::new(RecordingProcessControl {
            inner: process.control,
            pool: self.pool.clone(),
            intent: spec.intent,
            resource: resource.clone(),
        });
        let payload = match Effects::consume_stdin(&self.pool, spec.intent, &resource, master).await
        {
            Ok(payload) => payload,
            Err(error) => {
                // No effect may open the gate on unknown COMMIT/expiry/revocation.
                // Retain identity and stop evidence if provider cleanup is uncertain.
                process
                    .control
                    .stop(attune_common::workload_launch::LaunchStopReason::SourceRevoked)
                    .await?;
                return Err(error);
            }
        };
        crate::runtime::process_executor::execute_isolated_cancellable(
            process,
            payload,
            crate::runtime::process_executor::IsolatedCaptureOptions {
                timeout_secs: context.timeout,
                max_stdout_bytes: context.max_stdout_bytes,
                max_stderr_bytes: context.max_stderr_bytes,
                output_format: context.output_format,
                output_policy: context.output_policy,
                out_schema: context.out_schema.as_ref(),
                cancel_token: context.cancel_token,
                stdout_log_writer: context.stdout_log_writer,
                stderr_log_writer: context.stderr_log_writer,
            },
        )
        .await
        .map_err(|_| Error::execution("Isolated action capture failed"))
    }

    /// Execute the action using the runtime registry
    async fn execute_action(&self, context: ExecutionContext) -> Result<ExecutionResult> {
        debug!("Executing action: {}", context.action_ref);

        let runtime = self
            .runtime_registry
            .get_runtime(&context)
            .map_err(|e| Error::Internal(e.to_string()))?;

        let result = runtime
            .execute(context)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(result)
    }

    /// Store only the classified public result; raw output is a protected channel.
    async fn store_execution_artifacts(
        &self,
        execution_id: i64,
        public_result: &JsonValue,
    ) -> Result<()> {
        if let Some(data) = public_result.get("data") {
            self.artifact_manager
                .store_result(execution_id, data)
                .await?;
        }
        Ok(())
    }

    fn execution_log_artifact_ref(action_ref: &str, stream: ExecutionLogArtifactStream) -> String {
        format!("{}.{}.log", action_ref, stream.as_str())
    }

    async fn allocate_execution_log_artifacts(
        &self,
        execution: &Execution,
        action: &Action,
        preparation: &ExecutionPreparation,
    ) -> Result<ExecutionLogArtifacts> {
        // Registration establishes the attempt's authority even for silent code.
        // Per-stream metadata is deferred to the first nonempty write.
        let delivery = crate::completion::register(
            &self.pool,
            execution,
            self.worker_id.load(Ordering::Acquire),
            self.worker_instance,
        )
        .await?;
        let mut connection = self.pool.acquire().await?;
        let protected_delegation =
            attune_common::repositories::delegated_execution::DelegatedExecutionRepository::find(
                &mut connection,
                execution.id,
            )
            .await?
            .explicit;
        drop(connection);
        let policy = preparation.binding.output_policy;
        let protected_stdout = policy.stdout != attune_common::output_policy::StdoutPolicy::Public;
        let protected_stderr =
            policy.stderr == attune_common::output_policy::OutputVisibility::Secret;
        let log_encryption_key =
            if delivery.is_some() || protected_delegation || protected_stdout || protected_stderr {
                Some(
                    self.secret_manager
                        .encryption_key()
                        .ok_or_else(|| {
                            Error::PermissionDenied("Recorded logs require encryption".into())
                        })?
                        .to_owned(),
                )
            } else {
                None
            };
        let (stdout_mirror, stderr_mirror) =
            attune_common::runtime_log_mirror::select_mirror_sources(
                attune_common::runtime_log_mirror::RuntimeLogSource::Execution {
                    execution_id: execution.id,
                    parent_execution_id: execution.parent,
                    action_ref: execution.action_ref.clone(),
                    pack_ref: action.pack_ref.clone(),
                    trace_tag: execution.trace_tag.clone(),
                    worker_id: self.worker_id.load(Ordering::Acquire),
                    worker_name: self.worker_name.clone(),
                    worker_instance: self.worker_instance,
                },
                self.mirror_runtime_stdout_to_stdio
                    && policy.stdout != attune_common::output_policy::StdoutPolicy::Secret,
                self.mirror_runtime_stderr_to_stdio && !protected_stderr,
            );
        let execution = Arc::new(execution.clone());
        let make_writer = |stream: ExecutionLogArtifactStream,
                           mirror_source: Option<
            attune_common::runtime_log_mirror::RuntimeLogSource,
        >,
                           path: Arc<OnceLock<PathBuf>>| {
            let pool = self.pool.clone();
            let execution = execution.clone();
            let delivery = delivery.clone();
            let encryption_key = log_encryption_key.clone();
            let transport = self.transport.clone();
            let backend = if transport.transport_mode() == "volume" {
                LogStreamBackend::SharedFile
            } else {
                LogStreamBackend::ObjectSegments
            };
            let retention = self.effective_action_log_settings(action);
            let config = self.log_segment_config;
            let artifacts_dir = self.artifacts_dir.clone();
            let api_url = self.api_url.clone();
            let jwt = self.jwt_config.clone();
            let is_stdout = matches!(stream, ExecutionLogArtifactStream::Stdout);
            let max_bytes = if is_stdout {
                self.max_stdout_bytes
            } else {
                self.max_stderr_bytes
            };
            let initialized_mirror = mirror_source.clone();
            #[cfg(unix)]
            let async_delivery = if delivery.is_none()
                && protected_delegation
                && backend == LogStreamBackend::ObjectSegments
            {
                self.log_delivery.get().cloned()
            } else {
                None
            };
            BoundedLogFileWriter::lazy(
                move || async move {
                    let (file_path, version, transport) = if let Some(context) = &delivery {
                        let allocation = crate::completion::allocate_log_prepared(
                            &pool,
                            &execution,
                            context,
                            stream.as_str(),
                            backend,
                            retention.policy,
                            retention.limit,
                            config.max_segment_bytes as u64,
                            config.flush_interval_ms,
                            max_bytes,
                        )
                        .await?;
                        let path = allocation.path.clone();
                        let version = allocation.version;
                        let transport = crate::completion::CompletionTransport::from_prepared(
                            &pool,
                            context.clone(),
                            allocation,
                            transport,
                            &api_url,
                            &jwt,
                        )?;
                        (path, version, transport)
                    } else {
                        let (_, file_path, version) = Self::allocate_execution_log_artifact_with(
                            &pool,
                            &artifacts_dir,
                            transport.as_ref(),
                            &execution,
                            retention,
                            stream,
                            backend,
                        )
                        .await?;
                        (file_path, version, transport)
                    };
                    let writer = if backend == LogStreamBackend::SharedFile {
                        transport
                            .write_file(&file_path, b"", Some("text/plain"))
                            .await?;
                        BoundedLogFileWriter::from_shared_file_writer(
                            attune_common::log_stream::SharedFileLogWriter::new(
                                transport,
                                version,
                                file_path.clone(),
                                config,
                            ),
                            max_bytes,
                            is_stdout,
                            config.finalization_timeout_ms,
                        )
                    } else {
                        #[cfg(unix)]
                        if let Some(manager) = async_delivery {
                            let writer = BoundedLogFileWriter::from_spooling_writer(
                                manager.writer(execution.id, version, transport).await?,
                                max_bytes,
                                is_stdout,
                                config.finalization_timeout_ms,
                            )
                            .with_encryption(encryption_key.as_deref(), execution.id, version)
                            .with_mirror_source(initialized_mirror);
                            let relative = ValidatedRelativePath::new(&file_path)?;
                            path.set(artifacts_dir.join(relative.as_path()))
                                .map_err(|_| {
                                    Error::invalid_state("Log writer initialized twice")
                                })?;
                            return Ok(writer);
                        }
                        BoundedLogFileWriter::from_segmented_writer(
                            attune_common::log_stream::SegmentedLogWriter::new(
                                transport, version, config,
                            )?,
                            max_bytes,
                            is_stdout,
                        )
                    }
                    .with_encryption(encryption_key.as_deref(), execution.id, version)
                    .with_mirror_source(initialized_mirror);
                    let relative = ValidatedRelativePath::new(&file_path)?;
                    path.set(artifacts_dir.join(relative.as_path()))
                        .map_err(|_| Error::invalid_state("Log writer initialized twice"))?;
                    Ok(writer)
                },
                max_bytes,
                is_stdout,
                config.finalization_timeout_ms,
                mirror_source,
            )
        };
        Ok(ExecutionLogArtifacts {
            stdout_writer: make_writer(
                ExecutionLogArtifactStream::Stdout,
                stdout_mirror,
                preparation.stdout_path.clone(),
            ),
            stderr_writer: make_writer(
                ExecutionLogArtifactStream::Stderr,
                stderr_mirror,
                preparation.stderr_path.clone(),
            ),
        })
    }

    async fn allocate_execution_log_artifact_with(
        pool: &PgPool,
        artifacts_dir: &Path,
        transport: &dyn ArtifactFileTransport,
        execution: &Execution,
        retention: LogAllocationSettings,
        stream: ExecutionLogArtifactStream,
        backend: LogStreamBackend,
    ) -> Result<(PathBuf, String, i64)> {
        let artifact_ref = Self::execution_log_artifact_ref(&execution.action_ref, stream);
        let content_type = default_content_type_for_artifact(ArtifactType::FileText);
        let classification = classify_artifact(&artifact_ref, ArtifactType::FileText);

        let mut transaction = pool.begin().await?;
        let artifact = ArtifactRepository::create_or_get(
            &mut transaction,
            CreateArtifactInput {
                r#ref: artifact_ref.clone(),
                scope: OwnerType::Action,
                owner: execution.action_ref.clone(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification,
                retention_policy: retention.policy,
                retention_limit: retention.limit,
                name: Some(format!("{} {}", execution.action_ref, stream.as_str())),
                description: Some(format!(
                    "Captured {} for action '{}' (retention: {:?} {})",
                    stream.as_str(),
                    execution.action_ref,
                    retention.policy,
                    retention.limit
                )),
                content_type: Some(content_type.clone()),
                data: None,
            },
        )
        .await?;
        // Match version-writer lock order before changing parent metadata.
        let artifact = ArtifactRepository::find_by_id_for_update(&mut transaction, artifact.id)
            .await?
            .ok_or_else(|| Error::Internal("Log artifact disappeared during allocation".into()))?;
        if artifact.retention_policy != retention.policy
            || artifact.retention_limit != retention.limit
            || artifact.classification != classification
            || artifact.visibility != ArtifactVisibility::Private
        {
            ArtifactRepository::update(
                &mut *transaction,
                artifact.id,
                UpdateArtifactInput {
                    visibility: Some(ArtifactVisibility::Private),
                    classification: Some(classification),
                    retention_policy: Some(retention.policy),
                    retention_limit: Some(retention.limit),
                    ..Default::default()
                },
            )
            .await?;
        }
        let version = ArtifactVersionRepository::create_log_pending_with_conn(
            &mut transaction,
            artifact.id,
            &artifact.r#ref,
            backend,
            execution.id,
            serde_json::json!({
                "stream": stream.as_str(),
                "execution_id": execution.id,
            }),
        )
        .await?;
        attune_common::repositories::log_stream::LogStreamRepository::create_with_backend_with_conn(
            &mut transaction,
            version.id,
            backend,
            retention.segment_bytes,
            retention.flush_interval_ms,
        )
        .await?;
        if attune_common::repositories::delegated_execution::DelegatedExecutionRepository::find(
            &mut transaction,
            execution.id,
        )
        .await?
        .explicit
        {
            ArtifactVersionRepository::protect_delegated_logs(
                &mut transaction,
                execution.id,
                &[version.id],
            )
            .await?;
        }
        transaction.commit().await?;

        info!(
            execution_id = execution.id,
            artifact_id = artifact.id,
            artifact_ref = %artifact.r#ref,
            version_id = version.id,
            stream = stream.as_str(),
            classification = ?classification,
            "Allocated runtime log artifact version"
        );

        let file_path = version.file_path.ok_or_else(|| {
            Error::Internal(format!(
                "Allocated file-backed log version {} is missing file_path",
                version.id
            ))
        })?;

        // Ensure parent directories exist via transport. The file itself is
        // created by the log writer only when output bytes are written.
        transport
            .ensure_parent_dirs(&file_path)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to create log artifact directory for '{}': {}",
                    file_path, e
                ))
            })?;

        let relative = ValidatedRelativePath::new(&file_path)?;
        let full_path = artifacts_dir.join(relative.as_path());
        Ok((full_path, file_path, version.id))
    }

    async fn latest_execution_log_path(
        &self,
        execution_id: i64,
        stream: ExecutionLogArtifactStream,
    ) -> Result<Option<PathBuf>> {
        // Resolve action_ref by loading the execution row.
        let Some(execution) = ExecutionRepository::find_by_id(&self.pool, execution_id).await?
        else {
            return Ok(None);
        };
        let mut conn = self.pool.acquire().await?;
        let recorded =
            attune_common::repositories::execution_admission::ExecutionAdmissionRepository::is_recorded(&mut conn, execution_id,)
            .await?;
        drop(conn);
        let artifact_ref = if recorded {
            crate::completion::log_reference(&execution, stream.as_str())?
        } else {
            Self::execution_log_artifact_ref(&execution.action_ref, stream)
        };
        let Some(artifact) = ArtifactRepository::find_by_ref(&self.pool, &artifact_ref).await?
        else {
            return Ok(None);
        };
        // Find the version that was written by this specific execution (not just
        // the latest version of the artifact, since concurrent executions of the
        // same action would otherwise race for the "latest" slot).
        let Some(version) = ArtifactVersionRepository::find_by_artifact_and_execution(
            &self.pool,
            artifact.id,
            execution_id,
        )
        .await?
        else {
            return Ok(None);
        };
        let Some(file_path) = version.file_path else {
            return Ok(None);
        };
        let relative = ValidatedRelativePath::new(&file_path)?;
        Ok(Some(
            resolve_checked_path(&self.artifacts_dir, &relative).await?,
        ))
    }

    async fn retained_process(
        &self,
        intent: uuid::Uuid,
    ) -> Result<(
        attune_common::isolated_process::ProcessSpec,
        attune_common::isolated_process::ResourceIdentity,
    )> {
        use attune_common::repositories::execution_process::ExecutionProcessRepository as Effects;
        let master = self.secret_manager.encryption_key().ok_or_else(|| {
            Error::PermissionDenied("Process context encryption unavailable".into())
        })?;
        let spec = Effects::retained_spec(&self.pool, intent, master).await?;
        let resource = Effects::retained_resource(&self.pool, intent)
            .await?
            .ok_or_else(|| Error::invalid_state("Native resource settlement unavailable"))?;
        Ok((spec, resource))
    }

    async fn finalize_isolated_file_artifacts(
        &self,
        execution_id: i64,
        intent: uuid::Uuid,
    ) -> Result<()> {
        let versions = ArtifactVersionRepository::find_ordinary_file_versions_by_execution(
            &self.pool,
            execution_id,
        )
        .await?;
        if versions.is_empty() {
            return Ok(());
        }
        let backend = self
            .native_backend
            .as_ref()
            .ok_or_else(|| Error::invalid_state("Native artifact transfer effects unavailable"))?;
        let (spec, resource) = self.retained_process(intent).await?;
        if !matches!(spec.target, attune_common::workload_launch::LaunchTarget::Execution { execution, worker, .. }
            if execution == execution_id && worker == self.worker_id.load(Ordering::Acquire))
        {
            return Err(Error::PermissionDenied(
                "Native artifact owner mismatch".into(),
            ));
        }
        // The provider validates the entire retained tree against its operator
        // ceiling before returning any selected file, including ENOENT.
        for version in versions {
            let path = version.file_path.as_deref().ok_or_else(|| {
                Error::invalid_state("Allocated native artifact path unavailable")
            })?;
            let relative = ValidatedRelativePath::new(path)?;
            let mut conn = self.pool.acquire().await?;
            let recorded = attune_common::repositories::execution_admission::ExecutionAdmissionRepository::is_recorded(
                &mut conn, execution_id,
            ).await?;
            drop(conn);
            let transport = if recorded {
                crate::completion::CompletionTransport::for_version(
                    &self.pool,
                    version.id,
                    self.worker_id.load(Ordering::Acquire),
                    self.worker_instance,
                    self.transport.clone(),
                    &self.api_url,
                    &self.jwt_config,
                )
                .await?
            } else {
                // Even shared-volume managers transfer guest-private files
                // through the existing authenticated publication endpoint.
                // A local stat is not a receipt for this isolated stage.
                let provider = self.worker_tokens.clone().ok_or_else(|| {
                    Error::invalid_state("Native artifact delivery credential unavailable")
                })?;
                Arc::new(attune_common::artifact_transport::ApiTransport::new_with_worker_token_provider(
                    &self.api_url, provider, self.transport.base_dir(),
                )?)
            };
            if version.body_state == Some(attune_common::models::enums::ArtifactBodyState::Ready) {
                // API-capable actions may have uploaded directly; no guest
                // file is required for an already immutable published body.
                let expected = version
                    .size_bytes
                    .and_then(|n| u64::try_from(n).ok())
                    .ok_or_else(|| Error::invalid_state("Published artifact size unavailable"))?;
                if transport.complete_file(path).await? != Some(expected) {
                    return Err(Error::invalid_state(
                        "Published artifact acknowledgement mismatch",
                    ));
                }
                continue;
            }
            let source = backend
                .read_stopped_artifact(&spec, &resource, &relative, i64::MAX as u64)
                .await?
                .ok_or_else(|| Error::invalid_state("Allocated native artifact was not written"))?;
            crate::completion::publish_stopped_file(
                transport.as_ref(),
                source,
                path,
                version.content_type.as_deref(),
                i64::MAX as u64,
            )
            .await?;
            // The API commits exact-version metadata, output origins and
            // publication together. No unchecked post-receipt DB mutation.
        }
        Ok(())
    }

    /// Finalize file-backed artifacts after execution completes.
    ///
    /// Scans all artifact versions linked to this execution that have a `file_path`,
    /// stats each file via the transport, and updates `size_bytes` on both the
    /// version row and the parent artifact row.
    async fn finalize_file_artifacts(&self, execution_id: i64) -> Result<()> {
        let versions = ArtifactVersionRepository::find_ordinary_file_versions_by_execution(
            &self.pool,
            execution_id,
        )
        .await?;

        if versions.is_empty() {
            return Ok(());
        }

        info!(
            "Finalizing {} file-backed artifact version(s) for execution {}",
            versions.len(),
            execution_id,
        );

        // Track the latest version per artifact so we can update parent size_bytes
        let mut latest_size_per_artifact: HashMap<i64, (i32, i64)> = HashMap::new();

        for ver in &versions {
            let file_path = match &ver.file_path {
                Some(fp) => fp,
                None => continue,
            };

            let mut conn = self.pool.acquire().await?;
            // Ordinary file metadata outlives execution retention. This path
            // grants no runtime or disclosure authority; an absent owner only
            // permits finalizing the already allocated storage metadata.
            let recorded = if ExecutionRepository::find_by_id(&mut *conn, execution_id)
                .await?
                .is_some()
            {
                attune_common::repositories::execution_admission::ExecutionAdmissionRepository::is_recorded(&mut conn, execution_id).await?
            } else {
                false
            };
            drop(conn);
            let transport = if recorded {
                crate::completion::CompletionTransport::for_version(
                    &self.pool,
                    ver.id,
                    self.worker_id.load(Ordering::Acquire),
                    self.worker_instance,
                    self.transport.clone(),
                    &self.api_url,
                    &self.jwt_config,
                )
                .await?
            } else {
                self.transport.clone()
            };

            let local_synced_size = if transport.transport_mode() != "volume" {
                match sync_local_file_to_transport(
                    &self.artifacts_dir,
                    transport.as_ref(),
                    file_path,
                    ver.content_type.as_deref(),
                )
                .await
                {
                    Ok(size) => size,
                    Err(e) => {
                        warn!(
                            "Failed to copy local artifact file '{}' for version {} to {} transport: {}",
                            file_path,
                            ver.id,
                            self.transport.transport_mode(),
                            e,
                        );
                        continue;
                    }
                }
            } else {
                None
            };

            let completed_size = match transport.complete_file(file_path).await {
                Ok(size) => size.or(local_synced_size),
                Err(e) => {
                    warn!(
                        "Could not publish artifact file '{}' for version {}: {}",
                        file_path, ver.id, e
                    );
                    continue;
                }
            };
            let size_bytes = match completed_size {
                Some(size) => size as i64,
                None => match self.transport.file_size(file_path).await {
                    Ok(Some(size)) => size as i64,
                    Ok(None) => {
                        debug!(
                            "Removing unwritten artifact version {} (artifact {}): file='{}'",
                            ver.id, ver.artifact, file_path,
                        );
                        0
                    }
                    Err(e) => {
                        warn!(
                        "Could not stat artifact file '{}' for version {}: {}. Setting size_bytes=0.",
                        file_path,
                        ver.id,
                        e,
                    );
                        0
                    }
                },
            };

            // Recorded publication updates size metadata in its fenced API
            // transaction. Never follow it with an unchecked database mutation.
            if recorded {
                continue;
            }

            // Update the version row
            if let Err(e) =
                ArtifactVersionRepository::update_size_bytes(&self.pool, ver.id, size_bytes).await
            {
                warn!(
                    "Failed to update size_bytes for artifact version {}: {}",
                    ver.id, e,
                );
            }

            // Track the highest version number per artifact for parent update
            let entry = latest_size_per_artifact
                .entry(ver.artifact)
                .or_insert((ver.version, size_bytes));
            if ver.version > entry.0 {
                *entry = (ver.version, size_bytes);
            }

            debug!(
                "Finalized artifact version {} (artifact {}): file='{}', size={}",
                ver.id, ver.artifact, file_path, size_bytes,
            );
            info!(
                execution_id,
                artifact_id = ver.artifact,
                version_id = ver.id,
                file_path = %file_path,
                size_bytes,
                "Finalized runtime log artifact version"
            );
        }

        // Update parent artifact size_bytes to reflect the latest version's size
        for (artifact_id, (_version, size_bytes)) in &latest_size_per_artifact {
            if let Err(e) =
                ArtifactRepository::update_size_bytes(&self.pool, *artifact_id, *size_bytes).await
            {
                warn!(
                    "Failed to update size_bytes for artifact {}: {}",
                    artifact_id, e,
                );
            }
        }

        info!(
            "Finalized file-backed artifacts for execution {}: {} version(s), {} artifact(s)",
            execution_id,
            versions.len(),
            latest_size_per_artifact.len(),
        );

        Ok(())
    }

    /// Handle successful execution
    async fn handle_execution_success(
        &self,
        execution_id: i64,
        action: &Action,
        result: &ExecutionResult,
        preparation: &ExecutionPreparation,
    ) -> Result<ExecutionTransition> {
        info!(
            "Execution {} succeeded (exit_code={}, duration={}ms)",
            execution_id, result.exit_code, result.duration_ms
        );
        emit_runtime_log_truncation_events(execution_id, result);

        // Build comprehensive result with execution metadata
        let mut result_data = serde_json::json!({
            "exit_code": result.exit_code,
            "duration_ms": result.duration_ms,
            "succeeded": true,
        });

        // Include stdout content directly in result
        if !result.stdout.is_empty() {
            result_data["stdout"] = serde_json::json!(result.stdout);
        }

        // Include stderr log path only if stderr is non-empty and non-whitespace
        if !result.stderr.trim().is_empty() {
            if let Some(stderr_path) = preparation.stderr_path.get() {
                result_data["stderr_log"] = serde_json::json!(stderr_path.to_string_lossy());
            }
        }

        // Include parsed result if available
        if let Some(parsed_result) = &result.result {
            result_data["data"] = parsed_result.clone();
            Self::copy_reserved_queue_ack(&mut result_data, parsed_result);
        }

        record_runtime_log_capture(&mut result_data, result, Some(preparation.log_allocation()));
        self.persist_execution_result(
            execution_id,
            Some(action),
            result_data,
            Some(preparation),
            ExecutionStatus::Completed,
        )
        .await
    }

    fn copy_reserved_queue_ack(
        result_data: &mut serde_json::Value,
        parsed_result: &serde_json::Value,
    ) {
        if let Some(queue_ack) = parsed_result
            .as_object()
            .and_then(|object| object.get("queue_ack"))
        {
            result_data["queue_ack"] = queue_ack.clone();
        }
    }

    async fn persist_execution_result(
        &self,
        execution_id: i64,
        action: Option<&Action>,
        result_data: JsonValue,
        preparation: Option<&ExecutionPreparation>,
        status: ExecutionStatus,
    ) -> Result<ExecutionTransition> {
        let mut tx = self.pool.begin().await?;
        if let Some(intent) = preparation.and_then(|value| value.process_intent) {
            let master = self.secret_manager.encryption_key().ok_or_else(|| {
                Error::PermissionDenied("Process completion encryption unavailable".into())
            })?;
            if let Some(historical) = attune_common::repositories::execution_process::ExecutionProcessRepository::lock_completion(&mut tx,intent,master).await? {
                tx.commit().await?;
                return Ok(historical);
            }
        }
        let result_data = if action.is_some() {
            self.redact_execution_result_data_in(execution_id, &mut tx, result_data, preparation)
                .await?
        } else {
            result_data
        };
        let transition = ExecutionRepository::transition_worker(
            &mut *tx,
            execution_id,
            Some(self.worker_id.load(Ordering::Acquire)).filter(|id| *id > 0),
            status,
            Some(result_data.clone()),
        )
        .await?;
        tx.commit().await?;
        // This best-effort artifact contains only the classified public result.
        // Historical acknowledgement never repeats this write.
        if let Err(error) = self
            .store_execution_artifacts(execution_id, &result_data)
            .await
        {
            warn!(execution_id,%error,"Failed to store classified result artifact");
        }
        Ok(transition)
    }

    async fn redact_execution_result_data_in(
        &self,
        execution_id: i64,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        result_data: JsonValue,
        preparation: Option<&ExecutionPreparation>,
    ) -> Result<JsonValue> {
        let mut runtime_reads =
            attune_common::repositories::runtime_read::RuntimeReadRepository::close_execution(
                tx,
                execution_id,
            )
            .await?;
        let loaded_paths;
        let paths = match preparation.and_then(|context| context.parameter_secret_paths.as_deref())
        {
            Some(paths) => paths,
            None => {
                loaded_paths = ExecutionSecretValueRepository::find_stored_by_entity(
                    &mut **tx,
                    attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
                    execution_id,
                )
                .await?
                .into_iter()
                .map(|secret| secret.json_path)
                .collect::<Vec<_>>();
                &loaded_paths
            }
        };
        let loaded_binding;
        let binding = match preparation {
            Some(context) => &context.binding,
            None => {
                loaded_binding = attune_common::repositories::schema_contract::SchemaContractRepository::find_with_executor(
                    &mut **tx, attune_common::repositories::schema_contract::BindingOwner::Execution, execution_id,
                ).await?.ok_or_else(||Error::validation("Execution schema binding unavailable"))?;
                &loaded_binding
            }
        };
        let mut runtime_origins = attune_common::repositories::execution_key_read::ExecutionKeyReadRepository::origins_for_executions(
            &mut **tx, &[execution_id],
        ).await?;
        let runtime_origins = runtime_origins.remove(&execution_id).unwrap_or_default();
        runtime_reads.extend(
            runtime_origins
                .iter()
                .cloned()
                .map(attune_common::secret_provenance::SecretOrigin::Key),
        );
        let mut origins = attune_common::secret_provenance::SecretProvenance {
            origins: paths
                .iter()
                .map(
                    |path| attune_common::secret_provenance::SecretOrigin::Entity {
                        entity_type: attune_common::secret_values::ENTITY_EXECUTION_CONFIG.into(),
                        entity_id: execution_id,
                        path: path.clone(),
                    },
                )
                .collect(),
            templates: Vec::new(),
        };
        origins.origins.extend(runtime_reads);
        let classified = attune_common::output_classification::classify_action_output(
            result_data,
            binding.output.as_ref(),
            binding.output_format,
            binding.output_policy,
            execution_id,
            &origins,
        )?;
        let result_data = classified.public_result;
        let secret_inputs = classified.structured_secrets;
        let channel_inputs = classified.channel_secrets;
        if result_data.get("output_validation_warning").is_some() {
            warn!(
                execution_id,
                "Action output did not conform to its stored schema; successful status retained"
            );
        }
        if secret_inputs.is_empty() && channel_inputs.is_empty() {
            return Ok(result_data);
        }

        let encryption_key = self
            .secret_manager
            .encryption_key()
            .ok_or_else(|| Error::Internal("No encryption key configured".to_string()))?;
        let prepared = prepare_secret_values(secret_inputs, encryption_key)?;
        let channels = prepare_secret_values(channel_inputs, encryption_key)?;
        ExecutionSecretValueRepository::replace_entity_with_conn(
            tx,
            ENTITY_EXECUTION_RESULT,
            execution_id,
            &prepared,
        )
        .await?;
        ExecutionSecretValueRepository::replace_entity_with_conn(
            tx,
            attune_common::output_classification::ENTITY_EXECUTION_OUTPUT_CHANNEL,
            execution_id,
            &channels,
        )
        .await?;

        Ok(result_data)
    }

    async fn schema_binding(
        &self,
        execution_id: i64,
    ) -> Result<attune_common::repositories::schema_contract::ActionSchemaBinding> {
        use attune_common::repositories::schema_contract::{
            BindingOwner, SchemaContractRepository,
        };
        SchemaContractRepository::find_with_executor(
            &self.pool,
            BindingOwner::Execution,
            execution_id,
        )
        .await?
        .ok_or_else(|| {
            Error::Validation("Execution is missing its immutable schema binding".into())
        })
    }

    /// Handle failed execution
    async fn handle_execution_failure(
        &self,
        execution_id: i64,
        action: Option<&Action>,
        result: Option<&ExecutionResult>,
        error_message: Option<&str>,
        preparation: Option<&ExecutionPreparation>,
    ) -> Result<ExecutionTransition> {
        if let Some(r) = result {
            error!(
                execution_id,
                exit_code = r.exit_code,
                duration_ms = r.duration_ms,
                timed_out = r.timed_out,
                "Execution failed"
            );
            emit_runtime_log_truncation_events(execution_id, r);
        } else {
            error!(
                "Execution {} failed during preparation: {}",
                execution_id,
                error_message.unwrap_or("unknown error")
            );
        }

        let mut result_data = serde_json::json!({
            "succeeded": false,
        });

        // If we have execution result, include detailed information
        if let Some(exec_result) = result {
            result_data["exit_code"] = serde_json::json!(exec_result.exit_code);
            result_data["duration_ms"] = serde_json::json!(exec_result.duration_ms);

            if let Some(ref error) = exec_result.error {
                result_data["error"] = serde_json::json!(error);
            }

            // Include stdout content directly in result
            if !exec_result.stdout.is_empty() {
                result_data["stdout"] = serde_json::json!(exec_result.stdout);
            }

            // Include stderr log path only if stderr is non-empty and non-whitespace
            if !exec_result.stderr.trim().is_empty() {
                if let Some(stderr_path) = preparation.and_then(|context| context.stderr_path.get())
                {
                    result_data["stderr_log"] = serde_json::json!(stderr_path.to_string_lossy());
                }
            }

            // Add truncation warnings if applicable
            if exec_result.stdout_truncated {
                result_data["stdout_truncated"] = serde_json::json!(true);
                result_data["stdout_bytes_truncated"] =
                    serde_json::json!(exec_result.stdout_bytes_truncated);
            }
            if exec_result.stderr_truncated {
                result_data["stderr_truncated"] = serde_json::json!(true);
                result_data["stderr_bytes_truncated"] =
                    serde_json::json!(exec_result.stderr_bytes_truncated);
            }

            if let Some(parsed_result) = &exec_result.result {
                result_data["data"] = parsed_result.clone();
                Self::copy_reserved_queue_ack(&mut result_data, parsed_result);
            }
        } else {
            // No execution result available (early failure during setup/preparation)
            // This should be rare - most errors should be captured in ExecutionResult
            let err_msg = error_message.unwrap_or("Execution failed during preparation");
            result_data["error"] = serde_json::json!(err_msg);

            warn!(
                "Execution {} failed without ExecutionResult - {}: {}",
                execution_id, "early/catastrophic failure", err_msg
            );

            // Check if stderr log exists and is non-empty from artifact storage
            if let Some(stderr_path) = self
                .latest_execution_log_path(execution_id, ExecutionLogArtifactStream::Stderr)
                .await?
            {
                // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Log paths resolve from file-backed artifact metadata rooted under the configured artifact directory.
                if let Ok(contents) = tokio::fs::read_to_string(&stderr_path).await {
                    if !contents.trim().is_empty() {
                        result_data["stderr_log"] =
                            serde_json::json!(stderr_path.to_string_lossy());
                    }
                }
            }

            // Check if stdout log exists from artifact storage
            if let Some(stdout_path) = self
                .latest_execution_log_path(execution_id, ExecutionLogArtifactStream::Stdout)
                .await?
            {
                // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Log paths resolve from file-backed artifact metadata rooted under the configured artifact directory.
                if let Ok(contents) = tokio::fs::read_to_string(&stdout_path).await {
                    if !contents.is_empty() {
                        result_data["stdout"] = serde_json::json!(contents);
                    }
                }
            }
        }

        if let Some(result) = result {
            record_runtime_log_capture(
                &mut result_data,
                result,
                preparation.map(ExecutionPreparation::log_allocation),
            );
        }
        self.persist_execution_result(
            execution_id,
            action,
            result_data,
            preparation,
            ExecutionStatus::Failed,
        )
        .await
    }

    async fn handle_execution_cancelled(
        &self,
        execution_id: i64,
        action: &Action,
        result: &ExecutionResult,
        preparation: &ExecutionPreparation,
    ) -> Result<ExecutionTransition> {
        emit_runtime_log_truncation_events(execution_id, result);
        let mut result_data = serde_json::json!({
            "succeeded": false,
            "cancelled": true,
            "exit_code": result.exit_code,
            "duration_ms": result.duration_ms,
            "error": result.error.clone().unwrap_or_else(|| "Execution cancelled".to_string()),
        });

        if !result.stdout.is_empty() {
            result_data["stdout"] = serde_json::json!(result.stdout);
        }

        if !result.stderr.trim().is_empty() {
            if let Some(stderr_path) = preparation.stderr_path.get() {
                result_data["stderr_log"] = serde_json::json!(stderr_path.to_string_lossy());
            }
        }

        if result.stdout_truncated {
            result_data["stdout_truncated"] = serde_json::json!(true);
            result_data["stdout_bytes_truncated"] =
                serde_json::json!(result.stdout_bytes_truncated);
        }
        if result.stderr_truncated {
            result_data["stderr_truncated"] = serde_json::json!(true);
            result_data["stderr_bytes_truncated"] =
                serde_json::json!(result.stderr_bytes_truncated);
        }

        if let Some(parsed_result) = &result.result {
            result_data["data"] = parsed_result.clone();
            Self::copy_reserved_queue_ack(&mut result_data, parsed_result);
        }

        record_runtime_log_capture(&mut result_data, result, Some(preparation.log_allocation()));

        self.persist_execution_result(
            execution_id,
            Some(action),
            result_data,
            Some(preparation),
            ExecutionStatus::Cancelled,
        )
        .await
    }

    /// Persist a terminal `Timeout` status for an execution that exceeded its
    /// configured timeout and was terminated by the worker.
    async fn handle_execution_timeout(
        &self,
        execution_id: i64,
        action: &Action,
        result: &ExecutionResult,
        preparation: &ExecutionPreparation,
    ) -> Result<ExecutionTransition> {
        emit_runtime_log_truncation_events(execution_id, result);
        let mut result_data = serde_json::json!({
            "succeeded": false,
            "timed_out": true,
            "exit_code": result.exit_code,
            "duration_ms": result.duration_ms,
            "error": result
                .error
                .clone()
                .unwrap_or_else(|| "Execution timed out".to_string()),
        });

        if !result.stdout.is_empty() {
            result_data["stdout"] = serde_json::json!(result.stdout);
        }

        if !result.stderr.trim().is_empty() {
            if let Some(stderr_path) = preparation.stderr_path.get() {
                result_data["stderr_log"] = serde_json::json!(stderr_path.to_string_lossy());
            }
        }

        if result.stdout_truncated {
            result_data["stdout_truncated"] = serde_json::json!(true);
            result_data["stdout_bytes_truncated"] =
                serde_json::json!(result.stdout_bytes_truncated);
        }
        if result.stderr_truncated {
            result_data["stderr_truncated"] = serde_json::json!(true);
            result_data["stderr_bytes_truncated"] =
                serde_json::json!(result.stderr_bytes_truncated);
        }

        if let Some(parsed_result) = &result.result {
            result_data["data"] = parsed_result.clone();
            Self::copy_reserved_queue_ack(&mut result_data, parsed_result);
        }

        record_runtime_log_capture(&mut result_data, result, Some(preparation.log_allocation()));

        self.persist_execution_result(
            execution_id,
            Some(action),
            result_data,
            Some(preparation),
            ExecutionStatus::Timeout,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn recipe_inline_source_is_selected_at_the_action_boundary() {
        let config = super::RuntimeExecutionConfig { lifecycle: Some(serde_json::from_value(serde_json::json!({
            "tools":[{"name":"fixture","binaries":["tool"],"version_pattern":"(?<version>1)","primary":true}],
            "launch":{"command":["{tool_fixture}","{source_file}"]}
        })).unwrap()), ..Default::default() };
        assert_eq!(
            super::inline_action_source("console.log(1)", true, Some("Bun"), Some(&config), false)
                .as_deref(),
            Some("console.log(1)")
        );
        assert!(super::inline_action_source(
            "missing.ts",
            false,
            Some("Bun"),
            Some(&config),
            false
        )
        .is_none());
        assert!(
            super::inline_action_source("source.ts", true, Some("Bun"), Some(&config), true)
                .is_none()
        );
    }

    use super::*;
    use attune_common::artifact_transport::VolumeTransport;
    use attune_common::auth::jwt::{generate_execution_token, validate_token, JwtConfig};
    use attune_common::config::Config;
    use attune_common::models::enums::ArtifactClassification;
    use attune_common::repositories::Create;
    use attune_common::test_database::TestDatabase;
    use chrono::Utc;

    #[cfg(unix)]
    fn registration_executor(
        database: &TestDatabase,
        root: &Path,
        name: &str,
        tokens: Arc<attune_common::auth::WorkerTokenProvider>,
    ) -> Result<ActionExecutor> {
        let artifacts = root.join("artifacts");
        let jwt = JwtConfig {
            secret: "owned-registration-test-signing-key".into(),
            ..Default::default()
        };
        Ok(ActionExecutor::new(
            database.pool().clone(),
            RuntimeRegistry::new(),
            ArtifactManager::new(artifacts.clone()),
            SecretManager::new(
                database.pool().clone(),
                Some("owned-registration-encryption-key".into()),
            )?,
            1024,
            1024,
            false,
            false,
            name.into(),
            tokens.instance_id(),
            attune_common::log_stream::SegmentedLogConfig {
                initial_segment_bytes: 1024,
                max_segment_bytes: 16384,
                flush_interval_ms: 10,
                retry_max_attempts: 1,
                retry_attempt_timeout_ms: 500,
                retry_initial_backoff_ms: 1,
                retry_max_backoff_ms: 1,
                finalization_timeout_ms: 5000,
            },
            None,
            None,
            root.join("packs"),
            artifacts.clone(),
            root.join("runtimes"),
            "http://127.0.0.1:9".into(),
            jwt,
            Arc::new(
                attune_common::artifact_transport::ApiTransport::new_with_worker_token_provider(
                    "http://127.0.0.1:9",
                    tokens.clone(),
                    artifacts
                        .to_str()
                        .ok_or_else(|| Error::validation("fixture path"))?,
                )?,
            ),
        )
        .with_worker_token_provider(tokens))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn registration_custody_overlap_cannot_replace_the_live_incarnation() -> anyhow::Result<()>
    {
        use crate::registration::WorkerRegistration;
        use attune_common::{
            auth::WorkerTokenProvider,
            repositories::{runtime::WorkerRepository, FindById},
        };
        attune_common::auth::install_crypto_provider();
        let mut config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let name = format!("custody-owner-{}", uuid::Uuid::new_v4());
        config.worker = Some(serde_json::from_value(serde_json::json!({"name": name}))?);
        let database = TestDatabase::create(&config.database)
            .await?
            .with_cleanup_on_drop();
        let root = tempfile::tempdir()?;
        let delivery = attune_common::config::LogDeliveryConfig {
            spool_dir: Some(root.path().join("custody").to_string_lossy().into_owned()),
            ..Default::default()
        };
        let tokens = Arc::new(WorkerTokenProvider::new(1, "0", JwtConfig::default()));
        let first = registration_executor(&database, root.path(), &name, tokens.clone())?;
        let mut a = WorkerRegistration::new(database.pool().clone(), &config);
        a.add_capability(
            "log_delivery_instance".into(),
            serde_json::json!(tokens.instance_id()),
        );
        let id = first
            .register_worker_with_custody(&mut a, &delivery)
            .await?;
        let before = WorkerRepository::find_by_id(database.pool(), id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("registered worker"))?;
        let key_path = PathBuf::from(delivery.spool_dir.as_ref().unwrap())
            .join(id.to_string())
            .join("custody.key");
        let key = std::fs::read(&key_path)?;
        let next_tokens = Arc::new(WorkerTokenProvider::new(1, "0", JwtConfig::default()));
        let second = registration_executor(&database, root.path(), &name, next_tokens.clone())?;
        let mut b = WorkerRegistration::new(database.pool().clone(), &config);
        b.add_capability(
            "log_delivery_instance".into(),
            serde_json::json!(next_tokens.instance_id()),
        );
        anyhow::ensure!(
            second
                .register_worker_with_custody(&mut b, &delivery)
                .await
                .is_err(),
            "overlapping custody was admitted"
        );
        anyhow::ensure!(
            b.registered_incarnation().is_none(),
            "failed overlap acknowledged registration"
        );
        let after = WorkerRepository::find_by_id(database.pool(), id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("live worker"))?;
        anyhow::ensure!(
            before.meta == after.meta
                && before.capabilities == after.capabilities
                && before.status == after.status,
            "failed overlap mutated active registration"
        );
        drop(first);
        let next = second
            .register_worker_with_custody(&mut b, &delivery)
            .await?;
        anyhow::ensure!(
            next == id && a.registered_incarnation() != b.registered_incarnation(),
            "replacement identity not fenced"
        );
        anyhow::ensure!(
            std::fs::read(&key_path)? == key,
            "custody key rotated during replacement"
        );
        a.deregister().await?;
        let current = WorkerRepository::find_by_id(database.pool(), id)
            .await?
            .unwrap();
        anyhow::ensure!(
            serde_json::to_value(current.status)? == serde_json::json!("active"),
            "old deregistration closed replacement"
        );
        b.deregister().await?;
        drop(second);
        drop(a);
        drop(b);
        database.cleanup().await?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn registration_custody_failure_leaves_a_new_slot_inactive_and_unbound(
    ) -> anyhow::Result<()> {
        use crate::registration::WorkerRegistration;
        use attune_common::{
            auth::WorkerTokenProvider,
            models::{WorkerRole, WorkerType},
            repositories::{
                runtime::{CreateWorkerInput, WorkerRepository},
                FindById,
            },
        };
        attune_common::auth::install_crypto_provider();
        let mut config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let name = format!("custody-inactive-{}", uuid::Uuid::new_v4());
        config.worker = Some(serde_json::from_value(serde_json::json!({"name": name}))?);
        let database = TestDatabase::create(&config.database)
            .await?
            .with_cleanup_on_drop();
        let root = tempfile::tempdir()?;
        let delivery = attune_common::config::LogDeliveryConfig {
            spool_dir: Some(root.path().join("custody").to_string_lossy().into_owned()),
            ..Default::default()
        };
        let mut registration = WorkerRegistration::new(database.pool().clone(), &config);
        let id = registration.reserve_slot().await?;
        let owned = PathBuf::from(delivery.spool_dir.as_ref().unwrap()).join(id.to_string());
        std::fs::create_dir_all(&owned)?;
        let sentinel = root.path().join("unrelated-owned-sentinel");
        std::fs::write(&sentinel, b"unchanged")?;
        std::os::unix::fs::symlink(&sentinel, owned.join("owner.lock"))?;
        let tokens = Arc::new(WorkerTokenProvider::new(1, "0", JwtConfig::default()));
        let executor = registration_executor(&database, root.path(), &name, tokens)?;
        anyhow::ensure!(
            executor
                .register_worker_with_custody(&mut registration, &delivery)
                .await
                .is_err(),
            "invalid ownership accepted"
        );
        anyhow::ensure!(
            registration.registered_incarnation().is_none(),
            "failed ownership registered worker"
        );
        let row = WorkerRepository::find_by_id(database.pool(), id)
            .await?
            .unwrap();
        anyhow::ensure!(
            serde_json::to_value(row.status)? == serde_json::json!("inactive")
                && row.capabilities.is_none()
                && row
                    .meta
                    .as_ref()
                    .and_then(|v| v.get("registration_incarnation"))
                    .is_none(),
            "failed custody advertised authority"
        );
        anyhow::ensure!(
            std::fs::read(&sentinel)? == b"unchanged",
            "ownership failure changed unrelated file"
        );
        let wrong_name = CreateWorkerInput {
            name: format!("different-{name}"),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: None,
            capabilities: None,
            meta: None,
        };
        anyhow::ensure!(
            WorkerRepository::register_reserved_incarnation(
                database.pool(),
                wrong_name,
                WorkerRole::Action,
                uuid::Uuid::new_v4(),
                id
            )
            .await
            .is_err(),
            "reserved ID rebound to another name"
        );
        drop(executor);
        drop(registration);
        database.cleanup().await?;
        Ok(())
    }

    #[tokio::test]
    async fn physical_stop_is_not_blocked_by_durable_stop_lock_wait() -> anyhow::Result<()> {
        use attune_common::{
            isolated_process::{ProcessControl, ResourceIdentity},
            workload_launch::{LaunchExitOutcome, LaunchStopReason},
            workload_stdin::LaunchStdinPayload,
        };
        struct OwnedStop(Arc<tokio::sync::Notify>);
        #[async_trait::async_trait]
        impl ProcessControl for OwnedStop {
            async fn write_stdin_once(&self, _: LaunchStdinPayload) -> Result<()> {
                Err(Error::execution("test has no stdin"))
            }
            async fn wait(&self) -> Result<LaunchExitOutcome> {
                Err(Error::execution("test must stop"))
            }
            async fn stop(&self, _: LaunchStopReason) -> Result<LaunchExitOutcome> {
                self.0.notify_one();
                Ok(LaunchExitOutcome {
                    exit_code: 137,
                    oom_killed: false,
                })
            }
        }
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let database = TestDatabase::create(&config.database)
            .await?
            .with_cleanup_on_drop();
        let intent = uuid::Uuid::new_v4();
        let resource = ResourceIdentity::Kubernetes {
            cluster: "owned".into(),
            namespace: "owned".into(),
            pod_name: "owned".into(),
            pod_uid: uuid::Uuid::new_v4().to_string(),
        };
        let target = attune_common::workload_launch::LaunchTarget::Execution {
            execution: 1,
            worker: 1,
            worker_instance: uuid::Uuid::new_v4(),
            attempt: uuid::Uuid::new_v4(),
        };
        sqlx::query("INSERT INTO execution_process_effect(intent,target,encrypted_context,source_epochs,resource,create_submitted) VALUES($1,$2,'unused-test-context','[]',$3,TRUE)")
            .bind(intent).bind(sqlx::types::Json(target)).bind(sqlx::types::Json(&resource)).execute(database.pool()).await?;
        let mut barrier = database.pool().begin().await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let _: i64 = sqlx::query_scalar(
            "SELECT id FROM execution_process_effect WHERE intent=$1 FOR UPDATE",
        )
        .bind(intent)
        .fetch_one(&mut *barrier)
        .await?;
        let stopped = Arc::new(tokio::sync::Notify::new());
        let control = RecordingProcessControl {
            inner: Arc::new(OwnedStop(stopped.clone())),
            pool: database.pool().clone(),
            intent,
            resource,
        };
        let mut task = tokio::spawn(async move { control.stop(LaunchStopReason::Cancelled).await });
        let observed = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), stopped.notified()).await?;
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))")
                        .bind(blocker).fetch_one(database.pool()).await?;
                    if waiting { return Ok::<_, sqlx::Error>(()); }
                    tokio::task::yield_now().await;
                }
            }).await??;
            anyhow::ensure!(!task.is_finished(), "physical stop must not ACK durable settlement");
            Ok::<_, anyhow::Error>(())
        }.await;
        barrier.rollback().await?;
        if observed.is_err() {
            task.abort();
        }
        let joined = (&mut task).await;
        let check =
            async {
                observed?;
                let outcome = joined??;
                anyhow::ensure!(outcome.exit_code == 137);
                let (fenced, exit): (bool, Option<serde_json::Value>) = sqlx::query_as(
                "SELECT stop_requested,exit_outcome FROM execution_process_effect WHERE intent=$1")
                .bind(intent).fetch_one(database.pool()).await?;
                anyhow::ensure!(fenced && exit.is_some(), "stop/evidence were not committed");
                Ok::<_, anyhow::Error>(())
            }
            .await;
        database.cleanup().await?;
        check
    }

    #[test]
    fn successful_truncated_stdout_keeps_its_protection_reason() {
        use attune_common::{
            models::OutputFormat,
            output_classification::classify_action_output,
            output_policy::OutputPolicy,
            secret_provenance::SecretProvenance,
            value_schema::{compile_inline, SchemaContext},
        };
        let data = serde_json::json!({"token":"private","status":"visible"});
        let mut result = ExecutionResult::success(data.to_string(), Some(data.clone()), 1);
        result.stdout_truncated = true;
        result.stdout_bytes_truncated = 9;
        let mut envelope = serde_json::json!({"succeeded":true,"stdout":result.stdout,"data":data});
        record_runtime_log_capture(&mut envelope, &result, Some((true, false)));
        assert_eq!(envelope["stdout_bytes_truncated"], 9);
        let contract = compile_inline(
            "fixture.output",
            SchemaContext::FlatDocument,
            &serde_json::json!({"token":{"type":"string","secret":true}}),
        )
        .unwrap();
        let classified = classify_action_output(
            envelope,
            Some(&contract),
            OutputFormat::Json,
            OutputPolicy::default()
                .resolve(Some(&contract), OutputFormat::Json)
                .unwrap(),
            42,
            &SecretProvenance::default(),
        )
        .unwrap();
        assert_eq!(
            classified.public_result["output_channels"]["stdout"]["reason"],
            "truncated"
        );
        assert_eq!(classified.public_result["data"]["status"], "visible");
    }

    #[test]
    fn parsed_stdout_is_captured_even_when_the_result_stdout_string_is_empty() {
        let mut result =
            ExecutionResult::success(String::new(), Some(serde_json::json!({"answer":42})), 1);
        let mut metadata = serde_json::json!({});
        record_runtime_log_capture(&mut metadata, &result, Some((true, false)));
        assert_eq!(
            metadata["runtime_log_capture"],
            serde_json::json!({"stdout":"captured","stderr":"empty"})
        );
        record_runtime_log_capture(&mut metadata, &result, None);
        assert_eq!(metadata["runtime_log_capture"]["stdout"], "unknown");
        result.logs_incomplete = true;
        record_runtime_log_capture(&mut metadata, &result, Some((false, false)));
        assert_eq!(metadata["runtime_log_capture"]["stdout"], "incomplete");
        assert_eq!(metadata["runtime_log_capture"]["stderr"], "incomplete");
        result.log_capture_incomplete = Some((true, false));
        record_runtime_log_capture(&mut metadata, &result, Some((true, false)));
        assert_eq!(metadata["runtime_log_capture"]["stdout"], "incomplete");
        assert_eq!(metadata["runtime_log_capture"]["stderr"], "empty");
    }

    #[tokio::test]
    async fn concurrent_first_log_allocations_share_one_artifact_and_distinct_paths() {
        use attune_common::repositories::execution::CreateExecutionInput;
        use attune_common::repositories::List;
        use std::collections::HashSet;

        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = Config::load_from_file(&config_path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let directory = tempfile::tempdir().unwrap();
        let transport = VolumeTransport::new(directory.path().to_str().unwrap());
        let action_ref = "test.concurrent_log_first_allocation";
        let mut executions = Vec::new();
        for _ in 0..10 {
            executions.push(
                ExecutionRepository::create(
                    database.pool(),
                    CreateExecutionInput {
                        action_ref: action_ref.into(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap(),
            );
        }
        let failed = ActionExecutor::allocate_execution_log_artifact_with(
            database.pool(),
            directory.path(),
            &transport,
            &executions[0],
            LogAllocationSettings {
                policy: RetentionPolicyType::Versions,
                limit: 1,
                segment_bytes: 0,
                flush_interval_ms: 100,
            },
            ExecutionLogArtifactStream::Stderr,
            LogStreamBackend::SharedFile,
        )
        .await;
        assert!(
            failed.is_err(),
            "Invalid stream settings must fail allocation"
        );
        assert!(
            ArtifactRepository::list(database.pool())
                .await
                .unwrap()
                .iter()
                .all(|artifact| artifact.owner != action_ref),
            "Stream creation failure must roll back the artifact and version"
        );
        let start = tokio::sync::Barrier::new(executions.len());
        let allocations = futures::future::join_all(executions.iter().map(|execution| {
            let (database, directory, transport, start) =
                (&database, &directory, &transport, &start);
            async move {
                start.wait().await;
                ActionExecutor::allocate_execution_log_artifact_with(
                    database.pool(),
                    directory.path(),
                    transport,
                    execution,
                    LogAllocationSettings {
                        policy: RetentionPolicyType::Versions,
                        limit: 1,
                        segment_bytes: 65_536,
                        flush_interval_ms: 100,
                    },
                    ExecutionLogArtifactStream::Stderr,
                    LogStreamBackend::SharedFile,
                )
                .await
                .unwrap()
            }
        }))
        .await;
        let artifacts = ArtifactRepository::list(database.pool()).await.unwrap();
        assert_eq!(
            artifacts
                .iter()
                .filter(|artifact| artifact.owner == action_ref)
                .count(),
            1,
            "Concurrent first executions must not create duplicate log artifact rows"
        );
        let paths: HashSet<_> = allocations.iter().map(|(_, path, _)| path).collect();
        assert_eq!(paths.len(), executions.len());
        for (_, path, version) in allocations {
            assert!(
                attune_common::repositories::log_stream::LogStreamRepository::find_by_artifact_version(
                    database.pool(), version,
                ).await.unwrap().is_some(),
                "Every committed log version must have its stream",
            );
            assert_eq!(
                ArtifactVersionRepository::find_unique_by_file_path(database.pool(), &path)
                    .await
                    .unwrap()
                    .unwrap()
                    .id,
                version
            );
        }
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn finalization_preserves_zero_byte_file_version() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = Config::load_from_file(&config_path).expect("test config");
        let database = TestDatabase::create(&config.database)
            .await
            .expect("test database")
            .with_cleanup_on_drop();
        let directory = tempfile::tempdir().expect("temporary artifacts directory");
        let artifacts_dir = directory.path().join("artifacts");
        let transport = Arc::new(VolumeTransport::new(
            artifacts_dir.to_str().expect("UTF-8 artifact path"),
        ));
        let executor = ActionExecutor::new(
            database.pool().clone(),
            RuntimeRegistry::new(),
            ArtifactManager::new(artifacts_dir.clone()),
            SecretManager::new(database.pool().clone(), None).expect("secret manager"),
            1024,
            1024,
            false,
            false,
            "test-worker".to_string(),
            uuid::Uuid::nil(),
            attune_common::log_stream::SegmentedLogConfig {
                initial_segment_bytes: 1024,
                max_segment_bytes: 1024,
                flush_interval_ms: 1000,
                retry_max_attempts: 1,
                retry_attempt_timeout_ms: 100,
                retry_initial_backoff_ms: 1,
                retry_max_backoff_ms: 1,
                finalization_timeout_ms: 100,
            },
            None,
            None,
            directory.path().join("packs"),
            artifacts_dir,
            directory.path().join("runtimes"),
            "http://localhost:8080".to_string(),
            JwtConfig {
                secret: "worker-finalization-test-secret".to_string(),
                access_token_expiration: 300,
                refresh_token_expiration: 3600,
            },
            transport.clone(),
        );
        let artifact = ArtifactRepository::create(
            database.pool(),
            CreateArtifactInput {
                r#ref: "test.empty_worker_file".to_string(),
                scope: OwnerType::System,
                owner: "test".to_string(),
                r#type: ArtifactType::FileBinary,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::General,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: Some("application/octet-stream".to_string()),
                data: None,
            },
        )
        .await
        .expect("artifact");
        let execution_id = 4242;
        let version = ArtifactVersionRepository::create_file_backed(
            database.pool(),
            artifact.id,
            &artifact.r#ref,
            "application/octet-stream".to_string(),
            Some(execution_id),
            None,
            Some("test".to_string()),
        )
        .await
        .expect("file version");
        let file_path = version.file_path.as_deref().expect("file path");
        transport
            .write_file(file_path, b"", Some("application/octet-stream"))
            .await
            .expect("empty file");
        ArtifactVersionRepository::mark_body_ready(
            database.pool(),
            version.id,
            "e:empty-object",
            0,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        )
        .await
        .expect("mark ready")
        .expect("pending version became ready");
        assert_eq!(
            ArtifactVersionRepository::find_file_versions_by_execution(
                database.pool(),
                execution_id
            )
            .await
            .expect("execution versions")
            .len(),
            1
        );
        assert!(
            attune_common::repositories::log_stream::LogStreamRepository::find_by_artifact_version(
                database.pool(),
                version.id,
            )
            .await
            .expect("log stream lookup")
            .is_none()
        );

        executor
            .finalize_file_artifacts(execution_id)
            .await
            .expect("finalization");

        let finalized = ArtifactVersionRepository::find_by_id(database.pool(), version.id)
            .await
            .expect("version lookup")
            .expect("zero-byte version survives finalization");
        assert_eq!(finalized.size_bytes, Some(0));
        assert!(transport.file_exists(file_path).await.expect("file exists"));
    }

    #[test]
    fn test_resolve_execution_identity_uses_executor() {
        // SECURITY regression test: when an execution has its `executor`
        // populated, the resolved identity for the API token must be that
        // user — never the system identity.
        let user_id = 4242;
        let resolved = resolve_execution_identity(Some(user_id), 100).unwrap();
        assert_eq!(
            resolved, user_id,
            "resolve_execution_identity must return the execution's executor verbatim"
        );
        assert_ne!(
            resolved, 1,
            "must not silently elevate to the system identity"
        );
    }

    #[test]
    fn test_resolve_execution_identity_rejects_missing_owner() {
        assert!(resolve_execution_identity(None, 100).is_err());
    }

    #[test]
    fn test_resolve_timeout_prefers_snapshot() {
        // The snapshotted execution timeout always wins over every other tier.
        assert_eq!(
            resolve_timeout_seconds_inner(Some(45), Some(120), Some(300), 600),
            45
        );
    }

    #[test]
    fn test_resolve_timeout_falls_back_to_workflow_task() {
        // Legacy/incomplete rows with no snapshot fall back to the workflow
        // task timeout before the action default.
        assert_eq!(
            resolve_timeout_seconds_inner(None, Some(120), Some(300), 600),
            120
        );
    }

    #[test]
    fn test_resolve_timeout_falls_back_to_action_then_app_default() {
        assert_eq!(
            resolve_timeout_seconds_inner(None, None, Some(300), 600),
            300
        );
        assert_eq!(resolve_timeout_seconds_inner(None, None, None, 600), 600);
    }

    #[test]
    fn test_resolve_timeout_ignores_non_positive_and_out_of_range() {
        // Zero, negative, and otherwise invalid tiers are skipped so a bad
        // value never produces a zero/instant timeout.
        assert_eq!(
            resolve_timeout_seconds_inner(Some(0), Some(-5), Some(300), 600),
            300
        );
        assert_eq!(
            resolve_timeout_seconds_inner(Some(-1), None, None, 600),
            600
        );
    }

    #[test]
    fn test_execution_token_carries_resolved_identity() {
        // End-to-end check: the minted execution token's `sub` claim matches
        // the resolved identity. This is the property that downstream RBAC
        // relies on.
        attune_common::auth::crypto_provider::install();
        let config = JwtConfig {
            secret: "test_secret_for_executor_unit".to_string(),
            access_token_expiration: 3600,
            refresh_token_expiration: 86400,
        };
        let user_id = 7;
        let resolved = resolve_execution_identity(Some(user_id), 555).unwrap();
        let token = generate_execution_token(resolved, 555, "core.echo", &config, Some(60))
            .expect("token generation must succeed");
        let claims = validate_token(&token, &config).expect("token must validate");
        assert_eq!(claims.sub, user_id.to_string());
        assert_ne!(claims.sub, "1");
    }

    #[test]
    fn test_runtime_api_token_is_absent_without_permission_sets() {
        let config = JwtConfig {
            secret: "worker-runtime-cache-test-secret".to_string(),
            access_token_expiration: 3600,
            refresh_token_expiration: 86400,
        };

        let token = generate_runtime_api_token(
            42,
            555,
            "core.echo",
            &config,
            420,
            &[],
            &["core.echo".to_string()],
        )
        .expect("empty permission refs are a valid no-token configuration");

        assert!(token.is_none());
    }

    #[test]
    fn test_runtime_api_token_serializes_standard_cache_scope_context() {
        attune_common::auth::crypto_provider::install();
        let config = JwtConfig {
            secret: "worker-runtime-cache-test-secret".to_string(),
            access_token_expiration: 3600,
            refresh_token_expiration: 86400,
        };
        let permission_refs = vec!["standard".to_string()];
        let standard_action_refs = vec![
            "salesforce.lookup".to_string(),
            "orchestration.sync_accounts".to_string(),
        ];

        let token = generate_runtime_api_token(
            42,
            555,
            "salesforce.lookup",
            &config,
            420,
            &permission_refs,
            &standard_action_refs,
        )
        .expect("token generation")
        .expect("non-empty permission refs must produce a token");
        let claims = validate_token(&token, &config).expect("token validation");
        let metadata = claims.metadata.expect("execution token metadata");

        assert_eq!(
            metadata["permission_set_refs"],
            serde_json::json!(["standard"])
        );
        assert_eq!(
            metadata["standard_access_action_refs"],
            serde_json::json!(standard_action_refs)
        );
        assert_eq!(
            metadata["standard_access_pack_refs"],
            serde_json::json!(["salesforce", "orchestration"])
        );
    }

    #[test]
    fn test_parse_action_reference() {
        let action_ref = "mypack.myaction";
        let parts: Vec<&str> = action_ref.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "mypack");
        assert_eq!(parts[1], "myaction");
    }

    #[test]
    fn test_invalid_action_reference() {
        let action_ref = "invalid";
        let parts: Vec<&str> = action_ref.split('.').collect();
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn test_execution_log_artifact_ref() {
        assert_eq!(
            ActionExecutor::execution_log_artifact_ref(
                "mypack.deploy",
                ExecutionLogArtifactStream::Stdout
            ),
            "mypack.deploy.stdout.log"
        );
        assert_eq!(
            ActionExecutor::execution_log_artifact_ref(
                "mypack.deploy",
                ExecutionLogArtifactStream::Stderr
            ),
            "mypack.deploy.stderr.log"
        );
    }

    #[test]
    fn test_copy_reserved_queue_ack_preserves_data_shape() {
        let parsed_result = serde_json::json!({
            "message": "ok",
            "queue_ack": {
                "version": 1,
                "items": [
                    { "id": 42, "status": "completed" }
                ]
            }
        });
        let mut result_data = serde_json::json!({
            "succeeded": true,
            "data": parsed_result.clone()
        });

        ActionExecutor::copy_reserved_queue_ack(&mut result_data, &parsed_result);

        assert_eq!(result_data["data"], parsed_result);
        assert_eq!(result_data["queue_ack"], parsed_result["queue_ack"]);
    }

    #[test]
    fn test_copy_reserved_queue_ack_ignores_missing_ack() {
        let parsed_result = serde_json::json!({ "message": "ok" });
        let mut result_data = serde_json::json!({
            "succeeded": true,
            "data": parsed_result.clone()
        });

        ActionExecutor::copy_reserved_queue_ack(&mut result_data, &parsed_result);

        assert!(result_data.get("queue_ack").is_none());
    }

    #[test]
    fn test_copy_reserved_queue_ack_supports_failed_or_cancelled_results() {
        let parsed_result = serde_json::json!({
            "queue_ack": {
                "version": 1,
                "items": [
                    { "id": 7, "status": "retry" }
                ]
            }
        });
        let mut failed_result = serde_json::json!({
            "succeeded": false,
            "error": "boom"
        });
        let mut cancelled_result = serde_json::json!({
            "succeeded": false,
            "cancelled": true
        });

        ActionExecutor::copy_reserved_queue_ack(&mut failed_result, &parsed_result);
        ActionExecutor::copy_reserved_queue_ack(&mut cancelled_result, &parsed_result);

        assert_eq!(failed_result["queue_ack"], parsed_result["queue_ack"]);
        assert_eq!(cancelled_result["queue_ack"], parsed_result["queue_ack"]);
    }

    #[test]
    fn test_worker_runtime_versions_for_runtime_prefers_runtime_versions_capability() {
        let worker = Worker {
            id: 1,
            name: "worker-1".to_string(),
            worker_type: attune_common::models::WorkerType::Local,
            worker_role: attune_common::models::WorkerRole::Action,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: Some(serde_json::json!({
                "runtime_versions": {
                    "python": ["3.12.13"]
                },
                "detected_interpreters": [
                    { "name": "python", "path": "/usr/bin/python3", "version": "3.12.12" }
                ]
            })),
            meta: None,
            last_heartbeat: None,
            cordoned: false,
            cordon_reason: None,
            cordoned_by: None,
            cordoned_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        };
        let runtime = RuntimeModel {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            retired_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert_eq!(
            ActionExecutor::worker_runtime_versions_for_runtime(&worker, &runtime),
            vec!["3.12.13".to_string()]
        );
    }

    #[test]
    fn test_version_matches_worker_accepts_patch_level_for_minor_runtime_version() {
        let version = attune_common::models::RuntimeVersion {
            id: 1,
            runtime: 1,
            runtime_ref: "core.python".to_string(),
            version: "3.12".to_string(),
            version_major: Some(3),
            version_minor: Some(12),
            version_patch: None,
            execution_config: serde_json::json!({}),
            distributions: serde_json::json!({}),
            is_default: false,
            available: true,
            verified_at: None,
            meta: serde_json::json!({}),
            retired_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ActionExecutor::version_matches_worker(
            &version,
            &["3.12.13".to_string()]
        ));
        assert!(!ActionExecutor::version_matches_worker(
            &version,
            &["3.13.0".to_string()]
        ));
    }

    #[test]
    fn test_worker_runtime_versions_for_runtime_returns_empty_without_capabilities() {
        let worker = Worker {
            id: 1,
            name: "worker-1".to_string(),
            worker_type: attune_common::models::WorkerType::Local,
            worker_role: attune_common::models::WorkerRole::Action,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: None,
            meta: None,
            last_heartbeat: None,
            cordoned: false,
            cordon_reason: None,
            cordoned_by: None,
            cordoned_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        };
        let runtime = RuntimeModel {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            retired_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ActionExecutor::worker_runtime_versions_for_runtime(&worker, &runtime).is_empty());
    }
}
