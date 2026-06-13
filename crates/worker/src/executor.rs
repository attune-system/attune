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

use attune_common::artifact_transport::{sync_local_file_to_transport, ArtifactFileTransport};
use attune_common::auth::jwt::{
    generate_execution_token_with_permission_sets_and_standard_access, JwtConfig,
};
use attune_common::error::{Error, Result};
use attune_common::metadata_cache::{repositories::CachedMetadataRepository, MetadataCache};
use attune_common::models::runtime::RuntimeExecutionConfig;
use attune_common::models::{
    enums::{ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType},
    runtime::Runtime as RuntimeModel,
    Action, Execution, ExecutionStatus, Worker,
};
use attune_common::repositories::artifact::{
    default_content_type_for_artifact, ArtifactRepository, ArtifactVersionRepository,
    CreateArtifactInput, UpdateArtifactInput,
};
use attune_common::repositories::execution::{ExecutionRepository, UpdateExecutionInput};
use attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository;
use attune_common::repositories::runtime::WorkerRepository;
use attune_common::repositories::runtime::SELECT_COLUMNS as RUNTIME_SELECT_COLUMNS;
use attune_common::repositories::{Create, FindById, FindByRef, Update};
use attune_common::runtime_detection::normalize_runtime_name;
use attune_common::secret_values::{
    prepare_secret_values, redact_secret_parameters, ENTITY_EXECUTION_RESULT,
};
use attune_common::version_matching::{matches_constraint, select_best_version};
use std::path::PathBuf as StdPathBuf;

use serde_json::Value as JsonValue;
use sqlx::PgPool;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

use crate::artifacts::ArtifactManager;
use crate::runtime::{BoundedLogFileWriter, ExecutionContext, ExecutionResult, RuntimeRegistry};
use crate::secrets::SecretManager;

/// Action executor that orchestrates execution flow
pub struct ActionExecutor {
    pool: PgPool,
    metadata_cache: MetadataCache,
    runtime_registry: RuntimeRegistry,
    artifact_manager: ArtifactManager,
    secret_manager: SecretManager,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
    execution_log_retention_policy: RetentionPolicyType,
    execution_log_retention_limit: i32,
    packs_base_dir: PathBuf,
    artifacts_dir: PathBuf,
    runtime_envs_dir: PathBuf,
    api_url: String,
    jwt_config: JwtConfig,
    /// Transport abstraction for artifact file content.
    transport: Arc<dyn ArtifactFileTransport>,
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

/// System identity used as a security fallback when an execution has no
/// recorded triggering identity.
const SYSTEM_IDENTITY_ID: i64 = 1;

/// Default retention policy for per-execution stdout/stderr log artifacts.
/// The worker service passes configured values into `ActionExecutor::new`.
const DEFAULT_LOG_ARTIFACT_RETENTION_POLICY: RetentionPolicyType = RetentionPolicyType::Days;
const DEFAULT_LOG_ARTIFACT_RETENTION_LIMIT: i32 = 7;

/// Resolve the identity to embed in the execution-scoped API token (`sub`
/// claim).
///
/// Returns `executor` when set; otherwise logs a warning and falls back to
/// the system identity (1). A missing executor indicates a bug in one of the
/// execution-creation paths and is treated as a serious regression.
fn resolve_execution_identity(executor: Option<i64>, execution_id: i64) -> i64 {
    match executor {
        Some(id) => id,
        None => {
            warn!(
                "Execution {} has no executor identity set; falling back to system identity. \
                 This indicates a bug in an execution-creation path.",
                execution_id
            );
            SYSTEM_IDENTITY_ID
        }
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
fn resolve_execution_timeout_seconds(execution: &Execution, action: &Action) -> u64 {
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

struct ExecutionLogArtifacts {
    stdout_pending_full_path: PathBuf,
    stderr_pending_full_path: PathBuf,
}

struct StderrLogPromotion {
    handle: JoinHandle<Result<Option<PathBuf>>>,
    lock: Arc<AsyncMutex<()>>,
}

#[derive(Clone, Copy)]
struct LogRetentionSettings {
    policy: RetentionPolicyType,
    limit: i32,
}

impl ActionExecutor {
    /// Create a new action executor
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        metadata_cache: MetadataCache,
        runtime_registry: RuntimeRegistry,
        artifact_manager: ArtifactManager,
        secret_manager: SecretManager,
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
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
            pool,
            metadata_cache,
            runtime_registry,
            artifact_manager,
            secret_manager,
            max_stdout_bytes,
            max_stderr_bytes,
            execution_log_retention_policy: execution_log_retention_policy
                .unwrap_or(DEFAULT_LOG_ARTIFACT_RETENTION_POLICY),
            execution_log_retention_limit: execution_log_retention_limit
                .unwrap_or(DEFAULT_LOG_ARTIFACT_RETENTION_LIMIT),
            packs_base_dir,
            artifacts_dir,
            runtime_envs_dir,
            api_url,
            jwt_config,
            transport,
        }
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
        info!("Starting execution: {}", execution_id);

        // Update execution status to running
        if let Err(e) = self
            .update_execution_status(execution_id, ExecutionStatus::Running)
            .await
        {
            error!("Failed to update execution status to running: {}", e);
            return Err(e);
        }

        // Load execution from database
        let execution = self.load_execution(execution_id).await?;

        // Load action from database
        let action = self.load_action(&execution).await?;
        let log_retention = self.effective_action_log_retention(&action);

        // Prepare execution context
        let mut context = match self.prepare_execution_context(&execution, &action).await {
            Ok(ctx) => ctx,
            Err(e) => {
                error!("Failed to prepare execution context: {}", e);
                self.handle_execution_failure(
                    execution_id,
                    Some(&action),
                    None,
                    Some(&format!("Failed to prepare execution context: {}", e)),
                )
                .await?;
                return Err(e);
            }
        };

        // Attach the cancellation token so the process executor can monitor it
        context.cancel_token = Some(cancel_token.clone());

        let stdout_pending_path = context.stdout_log_path.clone();
        let stdout_promotion = stdout_pending_path.as_ref().map(|stdout_path| {
            self.spawn_log_promotion(
                &execution,
                stdout_path,
                ExecutionLogArtifactStream::Stdout,
                log_retention,
            )
        });
        let stderr_pending_path = context.stderr_log_path.clone();
        let stderr_promotion = stderr_pending_path.as_ref().map(|stderr_path| {
            self.spawn_log_promotion(
                &execution,
                stderr_path,
                ExecutionLogArtifactStream::Stderr,
                log_retention,
            )
        });

        // Execute the action
        // Note: execute_action should rarely return Err - most failures should be
        // captured in ExecutionResult with non-zero exit codes
        let result = match self.execute_action(context).await {
            Ok(result) => result,
            Err(e) => {
                error!("Action execution failed catastrophically: {}", e);
                if let (Some(stdout_path), Some(promotion)) =
                    (stdout_pending_path.as_deref(), stdout_promotion)
                {
                    self.finish_log_promotion(
                        &execution,
                        stdout_path,
                        ExecutionLogArtifactStream::Stdout,
                        promotion,
                        log_retention,
                    )
                    .await;
                }
                if let (Some(stderr_path), Some(promotion)) =
                    (stderr_pending_path.as_deref(), stderr_promotion)
                {
                    self.finish_log_promotion(
                        &execution,
                        stderr_path,
                        ExecutionLogArtifactStream::Stderr,
                        promotion,
                        log_retention,
                    )
                    .await;
                }
                if let Err(finalize_err) = self.finalize_file_artifacts(execution_id).await {
                    warn!(
                        "Failed to finalize file-backed artifacts for execution {} after catastrophic failure: {}",
                        execution_id, finalize_err
                    );
                }
                // This should only happen for unrecoverable errors like runtime not found
                self.handle_execution_failure(
                    execution_id,
                    Some(&action),
                    None,
                    Some(&format!("Action execution failed: {}", e)),
                )
                .await?;
                return Err(e);
            }
        };

        // Store artifacts
        if let Err(e) = self.store_execution_artifacts(execution_id, &result).await {
            warn!("Failed to store artifacts: {}", e);
            // Don't fail the execution just because artifact storage failed
        }

        if let (Some(stdout_path), Some(promotion)) =
            (stdout_pending_path.as_deref(), stdout_promotion)
        {
            self.finish_log_promotion(
                &execution,
                stdout_path,
                ExecutionLogArtifactStream::Stdout,
                promotion,
                log_retention,
            )
            .await;
        }
        if let (Some(stderr_path), Some(promotion)) =
            (stderr_pending_path.as_deref(), stderr_promotion)
        {
            self.finish_log_promotion(
                &execution,
                stderr_path,
                ExecutionLogArtifactStream::Stderr,
                promotion,
                log_retention,
            )
            .await;
        }

        // Finalize file-backed artifacts (stat files on disk and update size_bytes)
        if let Err(e) = self.finalize_file_artifacts(execution_id).await {
            warn!(
                "Failed to finalize file-backed artifacts for execution {}: {}",
                execution_id, e
            );
            // Don't fail the execution just because artifact finalization failed
        }

        // Update execution with result
        let is_success = result.is_success();
        debug!(
            "Execution {} result: exit_code={}, error={:?}, is_success={}",
            execution_id, result.exit_code, result.error, is_success
        );

        let was_cancelled = cancel_token.is_cancelled()
            || result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("cancelled"));

        if was_cancelled {
            self.handle_execution_cancelled(execution_id, &action, &result)
                .await?;
        } else if result.timed_out {
            self.handle_execution_timeout(execution_id, &action, &result)
                .await?;
        } else if is_success {
            self.handle_execution_success(execution_id, &action, &result)
                .await?;
        } else {
            self.handle_execution_failure(execution_id, Some(&action), Some(&result), None)
                .await?;
        }

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

        Ok(result)
    }

    /// Load execution from database
    async fn load_execution(&self, execution_id: i64) -> Result<Execution> {
        debug!("Loading execution: {}", execution_id);

        ExecutionRepository::find_by_id(&self.pool, execution_id)
            .await?
            .ok_or_else(|| Error::not_found("Execution", "id", execution_id.to_string()))
    }

    /// Load action from database using execution data
    async fn load_action(&self, execution: &Execution) -> Result<Action> {
        debug!("Loading action: {}", execution.action_ref);
        let cached = CachedMetadataRepository::new(&self.pool, &self.metadata_cache);

        // Try to load by action ID if available
        if let Some(action_id) = execution.action {
            if let Some(action) = cached.find_action_by_id(action_id).await? {
                return Ok(action);
            }
        }

        // Fallback: look up by the full qualified action ref directly
        if let Some(action) = cached.find_action_by_ref(&execution.action_ref).await? {
            return Ok(action);
        }

        Err(Error::not_found(
            "Action",
            "ref",
            execution.action_ref.clone(),
        ))
    }

    fn effective_action_log_retention(&self, action: &Action) -> LogRetentionSettings {
        LogRetentionSettings {
            policy: action
                .log_retention_policy
                .unwrap_or(self.execution_log_retention_policy),
            limit: action
                .log_retention_limit
                .unwrap_or(self.execution_log_retention_limit),
        }
    }

    /// Prepare execution context from execution and action data
    async fn prepare_execution_context(
        &self,
        execution: &Execution,
        action: &Action,
    ) -> Result<ExecutionContext> {
        debug!(
            "Preparing execution context for execution: {}",
            execution.id
        );

        // Extract parameters from execution config.
        // Config is always stored in flat format: the config object itself
        // is the parameters map (e.g. {"url": "...", "method": "GET"}).
        let mut parameters = HashMap::new();

        let restored_config = if let Some(config) = &execution.config {
            Some(
                self.secret_manager
                    .restore_execution_parameters(execution.id, config.clone())
                    .await?,
            )
        } else {
            None
        };

        if let Some(config) = &restored_config {
            debug!("Execution config present: {:?}", config);

            if let JsonValue::Object(map) = config {
                for (key, value) in map {
                    debug!("Adding parameter: {} = {:?}", key, value);
                    parameters.insert(key.clone(), value.clone());
                }
            } else {
                info!("Config is not an Object, cannot extract parameters");
            }
        } else {
            debug!("No execution config present");
        }

        debug!(
            "Extracted {} parameters: {:?}",
            parameters.len(),
            parameters
        );

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
            self.artifacts_dir.to_string_lossy().to_string(),
        );
        env.insert(
            "ATTUNE_RUNTIME_ENVS_DIR".to_string(),
            self.runtime_envs_dir.to_string_lossy().to_string(),
        );
        env.insert(
            "ATTUNE_PACK_REF".to_string(),
            execution
                .action_ref
                .split('.')
                .next()
                .unwrap_or(&execution.action_ref)
                .to_string(),
        );

        // Generate execution-scoped API token.
        //
        // SECURITY: The token's `sub` claim MUST reflect the identity that
        // triggered the execution. Otherwise actions executed on behalf of a
        // low-privilege user could call back into the API with system-level
        // privileges (privilege escalation).
        //
        // The triggering identity is recorded in `execution.executor` at
        // creation time by every execution-creation path (manual API,
        // enforcement processor, scheduler, queue dispatcher, retry manager,
        // workflow children). If `executor` is unset, that indicates a bug in
        // one of those paths; we fall back to the system identity (1) and log
        // a warning so the regression is visible.
        if execution.permission_set_refs.is_empty() {
            debug!(
                "Execution {} has no permission sets; omitting ATTUNE_API_TOKEN",
                execution.id
            );
        } else {
            let identity_id = resolve_execution_identity(execution.executor, execution.id);
            // Add a 60s grace period beyond the process timeout for cleanup and
            // callback reporting.
            let token_ttl = Some((execution_timeout + 60) as i64);
            let standard_access_action_refs = self.standard_access_action_refs(execution).await;
            match generate_execution_token_with_permission_sets_and_standard_access(
                identity_id,
                execution.id,
                &execution.action_ref,
                &self.jwt_config,
                token_ttl,
                &execution.permission_set_refs,
                &standard_access_action_refs,
            ) {
                Ok(token) => {
                    env.insert("ATTUNE_API_TOKEN".to_string(), token);
                }
                Err(e) => {
                    warn!(
                        "Failed to generate execution token for execution {}: {}. \
                         Actions that call back to the API will not authenticate.",
                        execution.id, e
                    );
                }
            }
        }

        // Add rule and trigger context if execution was triggered by enforcement
        if let Some(enforcement_id) = execution.enforcement {
            if let Ok(Some(enforcement)) = sqlx::query_as::<
                _,
                attune_common::models::event::Enforcement,
            >("SELECT * FROM enforcement WHERE id = $1")
            .bind(enforcement_id)
            .fetch_optional(&self.pool)
            .await
            {
                env.insert("ATTUNE_RULE".to_string(), enforcement.rule_ref);
                env.insert("ATTUNE_TRIGGER".to_string(), enforcement.trigger_ref);
            }
        }

        // Add context data as environment variables from config
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

        // Pack/action/system keys are still delivered through the dedicated
        // stdin secret channel. Execution-specific redacted parameters are
        // restored into `parameters` above and remain separate from these
        // ambient execution secrets.
        let secrets = match self.secret_manager.fetch_secrets_for_action(action).await {
            Ok(secrets) => {
                debug!(
                    "Fetched {} secrets for action {} (will be passed via stdin)",
                    secrets.len(),
                    action.r#ref
                );
                secrets
            }
            Err(e) => {
                warn!("Failed to fetch secrets for action {}: {}", action.r#ref, e);
                // Don't fail execution if ambient secret lookup fails; some
                // actions do not require pack/action secrets.
                HashMap::new()
            }
        };

        // Determine entry point from action
        let entry_point = action.entrypoint.clone();

        // Effective execution timeout resolved from execution.timeout_seconds
        // (snapshotted at creation) with defensive fallbacks for legacy rows.
        let timeout = Some(execution_timeout);

        // Load runtime information if specified
        let runtime_record = if let Some(runtime_id) = action.runtime {
            let query = format!(
                "SELECT {} FROM runtime WHERE id = $1",
                RUNTIME_SELECT_COLUMNS
            );
            match sqlx::query_as::<_, RuntimeModel>(&query)
                .bind(runtime_id)
                .fetch_optional(&self.pool)
                .await
            {
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

        let runtime_name = runtime_record.as_ref().map(|r| r.name.to_lowercase());

        // --- Runtime Version Resolution ---
        // If the action declares a runtime_version_constraint (e.g., ">=3.12"),
        // query all registered versions for this runtime and select the best
        // match. The selected version's execution_config overrides the parent
        // runtime's config so the ProcessRuntime uses a version-specific
        // interpreter binary, environment commands, etc.
        let (runtime_config_override, runtime_env_dir_suffix, selected_runtime_version) = self
            .resolve_runtime_version(&runtime_record, execution, action)
            .await;

        // Determine the pack directory for this action
        let pack_dir = self.packs_base_dir.join(&action.pack_ref);

        // Construct code_path for pack actions
        // Pack actions have their script files in packs/{pack_ref}/actions/{entrypoint}
        let code_path = if action.pack_ref.starts_with("core") || !action.is_adhoc {
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
        let code = if runtime_name.as_deref() == Some("shell") && code_path.is_none() {
            Some(entry_point.clone())
        } else {
            None
        };

        // Resolve the working directory from the runtime's execution_config.
        // The ProcessRuntime also does this internally, but setting it in the
        // context allows the executor to override if needed.
        let working_dir: Option<StdPathBuf> = if pack_dir.exists() {
            Some(pack_dir)
        } else {
            None
        };
        let log_artifacts = self.allocate_execution_log_artifacts(execution).await?;

        // Create transport-backed live log writers when not using volume transport.
        // For volume transport, the path-based BoundedLogFileWriter (created by
        // process_executor) writes directly to the shared filesystem.
        // For API transport, we create writers that stream bytes over HTTP.
        let (stdout_log_writer, stderr_log_writer) = if self.transport.transport_mode() != "volume"
        {
            let stdout_rel = log_artifacts
                .stdout_pending_full_path
                .strip_prefix(&self.artifacts_dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| format!("_pending/{}_stdout.log", execution.id));
            let stdout_writer = BoundedLogFileWriter::from_transport(
                self.transport.clone(),
                stdout_rel,
                self.max_stdout_bytes,
                true,
            );
            // Pending paths are promoted to real artifact versions only after
            // bytes are written.
            let stderr_rel = log_artifacts
                .stderr_pending_full_path
                .strip_prefix(&self.artifacts_dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| format!("_pending/{}_stderr.log", execution.id));
            let stderr_writer = BoundedLogFileWriter::from_transport(
                self.transport.clone(),
                stderr_rel,
                self.max_stderr_bytes,
                false,
            );
            (Some(stdout_writer), Some(stderr_writer))
        } else {
            (None, None)
        };

        let context = ExecutionContext {
            execution_id: execution.id,
            action_ref: execution.action_ref.clone(),
            parameters,
            env,
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
            stdout_log_path: Some(log_artifacts.stdout_pending_full_path),
            stderr_log_path: Some(log_artifacts.stderr_pending_full_path),
            stdout_log_writer,
            stderr_log_writer,
            parameter_delivery: action.parameter_delivery,
            parameter_format: action.parameter_format,
            output_format: action.output_format,
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
        let versions = match CachedMetadataRepository::new(&self.pool, &self.metadata_cache)
            .find_runtime_versions_by_runtime(runtime.id)
            .await
        {
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
        };

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
            .collect()
    }

    fn worker_runtime_versions_for_runtime(worker: &Worker, runtime: &RuntimeModel) -> Vec<String> {
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

    /// Store execution artifacts (logs, results)
    async fn store_execution_artifacts(
        &self,
        execution_id: i64,
        result: &ExecutionResult,
    ) -> Result<()> {
        debug!("Storing artifacts for execution: {}", execution_id);

        // Store result if available
        if let Some(result_data) = &result.result {
            self.artifact_manager
                .store_result(execution_id, result_data)
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
    ) -> Result<ExecutionLogArtifacts> {
        let stdout_pending_full_path = self
            .pending_execution_log_artifact_path(execution.id, ExecutionLogArtifactStream::Stdout);
        let stderr_pending_full_path = self
            .pending_execution_log_artifact_path(execution.id, ExecutionLogArtifactStream::Stderr);

        Ok(ExecutionLogArtifacts {
            stdout_pending_full_path,
            stderr_pending_full_path,
        })
    }

    fn pending_execution_log_artifact_path(
        &self,
        execution_id: i64,
        stream: ExecutionLogArtifactStream,
    ) -> PathBuf {
        self.artifacts_dir
            .join("_pending")
            .join("executions")
            .join(execution_id.to_string())
            .join(format!("{}.log", stream.as_str()))
    }

    fn spawn_log_promotion(
        &self,
        execution: &Execution,
        pending_path: &Path,
        stream: ExecutionLogArtifactStream,
        retention: LogRetentionSettings,
    ) -> StderrLogPromotion {
        let pool = self.pool.clone();
        let artifacts_dir = self.artifacts_dir.clone();
        let transport = Arc::clone(&self.transport);
        let execution = execution.clone();
        let pending_path = pending_path.to_path_buf();
        let lock = Arc::new(AsyncMutex::new(()));
        let task_lock = Arc::clone(&lock);

        let handle = tokio::spawn(async move {
            loop {
                sleep(Duration::from_millis(100)).await;
                let _guard = task_lock.lock().await;
                if let Some(final_path) = Self::persist_pending_log_artifact_if_written(
                    &pool,
                    &artifacts_dir,
                    transport.as_ref(),
                    &execution,
                    retention,
                    stream,
                    &pending_path,
                )
                .await?
                {
                    return Ok(Some(final_path));
                }
            }
        });

        StderrLogPromotion { handle, lock }
    }

    async fn finish_log_promotion(
        &self,
        execution: &Execution,
        pending_path: &Path,
        stream: ExecutionLogArtifactStream,
        promotion: StderrLogPromotion,
        retention: LogRetentionSettings,
    ) {
        {
            let _guard = promotion.lock.lock().await;
            if let Err(e) = Self::persist_pending_log_artifact_if_written(
                &self.pool,
                &self.artifacts_dir,
                self.transport.as_ref(),
                execution,
                retention,
                stream,
                pending_path,
            )
            .await
            {
                warn!(
                    "Failed to persist {} artifact for execution {}: {}",
                    stream.as_str(),
                    execution.id,
                    e
                );
            }
        }

        promotion.handle.abort();
        match promotion.handle.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => warn!(
                "Failed to persist live {} artifact for execution {}: {}",
                stream.as_str(),
                execution.id,
                e
            ),
            Err(e) if e.is_cancelled() => {}
            Err(e) => warn!(
                "Live {} artifact promotion task failed for execution {}: {}",
                stream.as_str(),
                execution.id,
                e
            ),
        }
    }

    async fn allocate_execution_log_artifact_with(
        pool: &PgPool,
        artifacts_dir: &Path,
        transport: &dyn ArtifactFileTransport,
        execution: &Execution,
        retention_policy: RetentionPolicyType,
        retention_limit: i32,
        stream: ExecutionLogArtifactStream,
    ) -> Result<(PathBuf, String)> {
        let artifact_ref = Self::execution_log_artifact_ref(&execution.action_ref, stream);
        let content_type = default_content_type_for_artifact(ArtifactType::FileText);

        let artifact = match ArtifactRepository::find_by_ref(pool, &artifact_ref).await? {
            Some(existing) => {
                if existing.retention_policy != retention_policy
                    || existing.retention_limit != retention_limit
                {
                    ArtifactRepository::update(
                        pool,
                        existing.id,
                        UpdateArtifactInput {
                            retention_policy: Some(retention_policy),
                            retention_limit: Some(retention_limit),
                            ..Default::default()
                        },
                    )
                    .await?
                } else {
                    existing
                }
            }
            None => {
                ArtifactRepository::create(
                    pool,
                    CreateArtifactInput {
                        r#ref: artifact_ref.clone(),
                        scope: OwnerType::Action,
                        owner: execution.action_ref.clone(),
                        r#type: ArtifactType::FileText,
                        visibility: ArtifactVisibility::Private,
                        retention_policy,
                        retention_limit,
                        name: Some(format!("{} {}", execution.action_ref, stream.as_str())),
                        description: Some(format!(
                            "Captured {} for action '{}' (retention: {:?} {})",
                            stream.as_str(),
                            execution.action_ref,
                            retention_policy,
                            retention_limit
                        )),
                        content_type: Some(content_type.clone()),
                        data: None,
                    },
                )
                .await?
            }
        };

        let version = ArtifactVersionRepository::create_file_backed(
            pool,
            artifact.id,
            &artifact.r#ref,
            content_type,
            Some(execution.id),
            Some(serde_json::json!({
                "stream": stream.as_str(),
                "execution_id": execution.id,
            })),
            Some("worker".to_string()),
        )
        .await?;

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

        let full_path = artifacts_dir.join(&file_path);
        Ok((full_path, file_path))
    }

    async fn persist_pending_log_artifact_if_written(
        pool: &PgPool,
        artifacts_dir: &Path,
        transport: &dyn ArtifactFileTransport,
        execution: &Execution,
        retention: LogRetentionSettings,
        stream: ExecutionLogArtifactStream,
        pending_path: &Path,
    ) -> Result<Option<PathBuf>> {
        let pending_relative_path = pending_path
            .strip_prefix(artifacts_dir)
            .ok()
            .map(|path| path.to_string_lossy().to_string());

        let metadata = match tokio::fs::metadata(pending_path).await {
            Ok(metadata) => Some(metadata),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => {
                return Err(Error::Internal(format!(
                    "Failed to stat pending stderr log '{}': {}",
                    pending_path.display(),
                    e
                )));
            }
        };

        let content = if let Some(metadata) = metadata {
            if metadata.len() == 0 {
                let _ = tokio::fs::remove_file(pending_path).await;
                Self::remove_empty_pending_parent_dirs(pending_path).await;
                return Ok(None);
            }

            tokio::fs::read(pending_path).await.map_err(|e| {
                Error::Internal(format!(
                    "Failed to read pending {} log '{}': {}",
                    stream.as_str(),
                    pending_path.display(),
                    e
                ))
            })?
        } else if transport.transport_mode() != "volume" {
            let Some(pending_relative_path) = pending_relative_path.as_deref() else {
                return Ok(None);
            };
            match transport.file_size(pending_relative_path).await {
                Ok(Some(size)) if size > 0 => transport
                    .read_file(pending_relative_path)
                    .await
                    .map_err(|e| {
                        Error::Internal(format!(
                            "Failed to read pending {} log '{}' from {} transport: {}",
                            stream.as_str(),
                            pending_relative_path,
                            transport.transport_mode(),
                            e
                        ))
                    })?,
                Ok(_) => return Ok(None),
                Err(e) => {
                    return Err(Error::Internal(format!(
                        "Failed to stat pending {} log '{}' from {} transport: {}",
                        stream.as_str(),
                        pending_relative_path,
                        transport.transport_mode(),
                        e
                    )));
                }
            }
        } else {
            return Ok(None);
        };

        let (final_path, relative_path) = Self::allocate_execution_log_artifact_with(
            pool,
            artifacts_dir,
            transport,
            execution,
            retention.policy,
            retention.limit,
            stream,
        )
        .await?;

        transport
            .write_file(&relative_path, &content, Some("text/plain"))
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to write {} log to '{}': {}",
                    stream.as_str(),
                    relative_path,
                    e
                ))
            })?;
        if pending_path.exists() {
            let _ = tokio::fs::remove_file(pending_path).await;
            Self::remove_empty_pending_parent_dirs(pending_path).await;
        } else if let Some(pending_relative_path) = pending_relative_path {
            let _ = transport.delete_file(&pending_relative_path).await;
        }

        Ok(Some(final_path))
    }

    async fn remove_empty_pending_parent_dirs(path: &Path) {
        let Some(execution_dir) = path.parent() else {
            return;
        };
        let _ = tokio::fs::remove_dir(execution_dir).await;

        let Some(executions_dir) = execution_dir.parent() else {
            return;
        };
        let _ = tokio::fs::remove_dir(executions_dir).await;

        let Some(pending_dir) = executions_dir.parent() else {
            return;
        };
        let _ = tokio::fs::remove_dir(pending_dir).await;
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
        let artifact_ref = Self::execution_log_artifact_ref(&execution.action_ref, stream);
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
        Ok(version.file_path.map(|path| self.artifacts_dir.join(path)))
    }

    /// Finalize file-backed artifacts after execution completes.
    ///
    /// Scans all artifact versions linked to this execution that have a `file_path`,
    /// stats each file via the transport, and updates `size_bytes` on both the
    /// version row and the parent artifact row.
    async fn finalize_file_artifacts(&self, execution_id: i64) -> Result<()> {
        let versions =
            ArtifactVersionRepository::find_file_versions_by_execution(&self.pool, execution_id)
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

            let local_synced_size = if self.transport.transport_mode() != "volume" {
                match sync_local_file_to_transport(
                    &self.artifacts_dir,
                    self.transport.as_ref(),
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

            let size_bytes = match local_synced_size {
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

            // If the file is empty, delete the version and clean up the file.
            if size_bytes == 0 {
                debug!(
                    "Removing empty artifact version {} (artifact {}): file='{}'",
                    ver.id, ver.artifact, file_path,
                );
                let _ = self.transport.delete_file(file_path).await;
                if let Err(e) = ArtifactVersionRepository::delete(&self.pool, ver.id).await {
                    warn!("Failed to delete empty artifact version {}: {}", ver.id, e,);
                }
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
    ) -> Result<()> {
        info!(
            "Execution {} succeeded (exit_code={}, duration={}ms)",
            execution_id, result.exit_code, result.duration_ms
        );

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
            if let Some(stderr_path) = self
                .latest_execution_log_path(execution_id, ExecutionLogArtifactStream::Stderr)
                .await?
            {
                result_data["stderr_log"] = serde_json::json!(stderr_path.to_string_lossy());
            }
        }

        // Include parsed result if available
        if let Some(parsed_result) = &result.result {
            result_data["data"] = parsed_result.clone();
            Self::copy_reserved_queue_ack(&mut result_data, parsed_result);
        }

        result_data = self
            .redact_execution_result_data(execution_id, action, result_data)
            .await?;

        let input = UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(result_data),
            ..Default::default()
        };

        ExecutionRepository::update(&self.pool, execution_id, input).await?;

        Ok(())
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

    async fn redact_execution_result_data(
        &self,
        execution_id: i64,
        action: &Action,
        mut result_data: JsonValue,
    ) -> Result<JsonValue> {
        let Some(parsed_result) = result_data.get("data").cloned() else {
            return Ok(result_data);
        };

        let (redacted_data, mut secret_inputs) =
            redact_secret_parameters(parsed_result, action.out_schema.as_ref());
        if secret_inputs.is_empty() {
            return Ok(result_data);
        }

        for input in &mut secret_inputs {
            input.json_path = format!("/data{}", input.json_path);
        }
        result_data["data"] = redacted_data;

        let encryption_key = self
            .secret_manager
            .encryption_key()
            .ok_or_else(|| Error::Internal("No encryption key configured".to_string()))?;
        let prepared = prepare_secret_values(secret_inputs, encryption_key)?;
        ExecutionSecretValueRepository::upsert_many(
            &self.pool,
            ENTITY_EXECUTION_RESULT,
            execution_id,
            &prepared,
        )
        .await?;

        Ok(result_data)
    }

    /// Handle failed execution
    async fn handle_execution_failure(
        &self,
        execution_id: i64,
        action: Option<&Action>,
        result: Option<&ExecutionResult>,
        error_message: Option<&str>,
    ) -> Result<()> {
        if let Some(r) = result {
            error!(
                "Execution {} failed (exit_code={}, error={:?}, duration={}ms)",
                execution_id, r.exit_code, r.error, r.duration_ms
            );
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
                if let Some(stderr_path) = self
                    .latest_execution_log_path(execution_id, ExecutionLogArtifactStream::Stderr)
                    .await?
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

        if let Some(action) = action {
            result_data = self
                .redact_execution_result_data(execution_id, action, result_data)
                .await?;
        }

        let input = UpdateExecutionInput {
            status: Some(ExecutionStatus::Failed),
            result: Some(result_data),
            ..Default::default()
        };

        ExecutionRepository::update(&self.pool, execution_id, input).await?;

        Ok(())
    }

    async fn handle_execution_cancelled(
        &self,
        execution_id: i64,
        action: &Action,
        result: &ExecutionResult,
    ) -> Result<()> {
        let mut result_data = serde_json::json!({
            "succeeded": false,
            "cancelled": true,
            "exit_code": result.exit_code,
            "duration_ms": result.duration_ms,
            "error": result.error.clone().unwrap_or_else(|| "Execution cancelled by user".to_string()),
        });

        if !result.stdout.is_empty() {
            result_data["stdout"] = serde_json::json!(result.stdout);
        }

        if !result.stderr.trim().is_empty() {
            if let Some(stderr_path) = self
                .latest_execution_log_path(execution_id, ExecutionLogArtifactStream::Stderr)
                .await?
            {
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

        result_data = self
            .redact_execution_result_data(execution_id, action, result_data)
            .await?;

        let input = UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            result: Some(result_data),
            ..Default::default()
        };

        ExecutionRepository::update(&self.pool, execution_id, input).await?;

        Ok(())
    }

    /// Persist a terminal `Timeout` status for an execution that exceeded its
    /// configured timeout and was terminated by the worker.
    async fn handle_execution_timeout(
        &self,
        execution_id: i64,
        action: &Action,
        result: &ExecutionResult,
    ) -> Result<()> {
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
            if let Some(stderr_path) = self
                .latest_execution_log_path(execution_id, ExecutionLogArtifactStream::Stderr)
                .await?
            {
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

        result_data = self
            .redact_execution_result_data(execution_id, action, result_data)
            .await?;

        let input = UpdateExecutionInput {
            status: Some(ExecutionStatus::Timeout),
            result: Some(result_data),
            ..Default::default()
        };

        ExecutionRepository::update(&self.pool, execution_id, input).await?;

        Ok(())
    }

    /// Update execution status
    async fn update_execution_status(
        &self,
        execution_id: i64,
        status: ExecutionStatus,
    ) -> Result<()> {
        debug!(
            "Updating execution {} status to: {:?}",
            execution_id, status
        );

        let started_at = if status == ExecutionStatus::Running {
            Some(chrono::Utc::now())
        } else {
            None
        };

        let input = UpdateExecutionInput {
            status: Some(status),
            started_at,
            ..Default::default()
        };

        let execution = ExecutionRepository::find_by_id(&self.pool, execution_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Execution {} not found", execution_id))?;

        ExecutionRepository::update_loaded(&self.pool, &execution, input).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::auth::jwt::{generate_execution_token, validate_token, JwtConfig};
    use chrono::Utc;

    #[test]
    fn test_resolve_execution_identity_uses_executor() {
        // SECURITY regression test: when an execution has its `executor`
        // populated, the resolved identity for the API token must be that
        // user — never the system identity.
        let user_id = 4242;
        let resolved = resolve_execution_identity(Some(user_id), 100);
        assert_eq!(
            resolved, user_id,
            "resolve_execution_identity must return the execution's executor verbatim"
        );
        assert_ne!(
            resolved, SYSTEM_IDENTITY_ID,
            "must not silently elevate to the system identity"
        );
    }

    #[test]
    fn test_resolve_execution_identity_falls_back_when_missing() {
        // When an execution-creation path forgets to populate `executor` we
        // fall back to the system identity; this is logged as a warning.
        let resolved = resolve_execution_identity(None, 100);
        assert_eq!(resolved, SYSTEM_IDENTITY_ID);
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
        let resolved = resolve_execution_identity(Some(user_id), 555);
        let token = generate_execution_token(resolved, 555, "core.echo", &config, Some(60))
            .expect("token generation must succeed");
        let claims = validate_token(&token, &config).expect("token must validate");
        assert_eq!(claims.sub, user_id.to_string());
        assert_ne!(claims.sub, SYSTEM_IDENTITY_ID.to_string());
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
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ActionExecutor::worker_runtime_versions_for_runtime(&worker, &runtime).is_empty());
    }
}
