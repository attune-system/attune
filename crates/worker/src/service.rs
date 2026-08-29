//! Worker Service Module
//!
//! Main service orchestration for the Attune Worker Service.
//! Manages worker registration, heartbeat, message consumption, and action execution.
//!
//! ## Startup Sequence
//!
//! 1. Connect to database and message queue
//! 2. Load runtimes from database → create `ProcessRuntime` instances
//! 3. Register worker and set up MQ infrastructure
//! 4. **Verify runtime versions** — run verification commands for each registered
//!    `RuntimeVersion` to determine which are available on this host/container
//! 5. **Set up runtime environments** — create per-version environments for packs
//! 6. Start heartbeat, execution consumer, pack registration consumer, and cancel consumer

use attune_common::auth::WorkerTokenProvider;
use attune_common::config::Config;
use attune_common::db::Database;
use attune_common::error::{Error, Result};
use attune_common::models::{pack_install::PackInstallStatus, ExecutionStatus};
use attune_common::mq::{
    config::MessageQueueConfig as MqConfig, routing_keys, ActionChangedPayload, Connection,
    Consumer, ConsumerConfig, ExecutionCancelRequestedPayload, ExecutionCompletedPayload,
    ExecutionStatusChangedPayload, MessageEnvelope, MessageType, MqError, PackChangedPayload,
    PackDeletedPayload, PackRegisteredPayload, PackTestRequestedPayload, Publisher,
    PublisherConfig, RuntimeChangedPayload,
};
use attune_common::repositories::{
    execution::ExecutionRepository, FindById, FindByRef, PackInstallRepository, PackRepository,
    PackTestRepository,
};
use attune_common::runtime_detection::runtime_aliases_match_filter;
use attune_common::schema::RefValidator;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::artifacts::ArtifactManager;
use crate::env_setup;
use crate::executor::ActionExecutor;
use crate::heartbeat::HeartbeatManager;
use crate::registration::WorkerRegistration;
use crate::runtime::local::LocalRuntime;
use crate::runtime::native::NativeRuntime;
use crate::runtime::process::ProcessRuntime;
use crate::runtime::RuntimeRegistry;
use crate::runtime_detect::DetectedRuntime;
use crate::secrets::SecretManager;
use crate::version_verify;

use attune_common::repositories::runtime::RuntimeRepository;
use attune_common::repositories::List;

/// Controls how the worker initializes its runtime environment.
///
/// The standard `attune-worker` binary uses `Worker` mode with proactive
/// setup at startup, while the `attune-agent` binary uses `Agent` mode
/// with lazy (on-demand) initialization.
#[derive(Debug, Clone)]
pub enum StartupMode {
    /// Full worker mode: proactive environment setup, full version
    /// verification sweep at startup. Used by `attune-worker`.
    Worker,

    /// Agent mode: lazy environment setup (on first use), on-demand
    /// version verification, auto-detected runtimes. Used by `attune-agent`.
    Agent {
        /// Runtimes detected by the auto-detection module.
        detected_runtimes: Vec<DetectedRuntime>,
    },
}

/// Message payload for execution.scheduled events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionScheduledPayload {
    pub execution_id: i64,
    pub action_ref: String,
    pub worker_id: i64,
}

fn filter_runtime_names_for_worker(
    runtime_names: &[String],
    worker_filter: Option<&[String]>,
) -> Vec<String> {
    match worker_filter {
        Some(worker_filter) => runtime_names
            .iter()
            .filter(|name| runtime_aliases_match_filter(&[(*name).clone()], worker_filter))
            .cloned()
            .collect(),
        None => runtime_names.to_vec(),
    }
}

async fn refresh_runtime_version_capabilities_for_registration(
    db_pool: &PgPool,
    runtime_filter: Option<&[String]>,
    registration: &Arc<RwLock<WorkerRegistration>>,
) {
    let capability =
        version_verify::collect_runtime_versions_capability(db_pool, runtime_filter).await;

    let mut reg = registration.write().await;
    reg.add_capability(
        version_verify::RUNTIME_VERSIONS_CAPABILITY_KEY.to_string(),
        capability,
    );

    if let Err(e) = reg.update_capabilities().await {
        warn!(
            "Failed to refresh worker runtime version capabilities: {}",
            e
        );
    }
}

/// Worker service that manages execution lifecycle
pub struct WorkerService {
    #[allow(dead_code)]
    config: Config,
    db_pool: PgPool,
    registration: Arc<RwLock<WorkerRegistration>>,
    heartbeat: Arc<HeartbeatManager>,
    executor: Arc<ActionExecutor>,
    mq_connection: Arc<Connection>,
    publisher: Arc<Publisher>,
    consumer: Option<Arc<Consumer>>,
    consumer_handle: Option<JoinHandle<()>>,
    task_reaper_handle: Option<JoinHandle<()>>,
    pack_consumer: Option<Arc<Consumer>>,
    pack_consumer_handle: Option<JoinHandle<()>>,
    pack_test_consumer: Option<Arc<Consumer>>,
    pack_test_consumer_handle: Option<JoinHandle<()>>,
    cancel_consumer: Option<Arc<Consumer>>,
    cancel_consumer_handle: Option<JoinHandle<()>>,
    metadata_consumer_handle: Option<JoinHandle<()>>,
    worker_id: Option<i64>,
    /// Runtime filter derived from ATTUNE_WORKER_RUNTIMES
    runtime_filter: Option<Vec<String>>,
    /// Base directory for pack files
    packs_base_dir: PathBuf,
    /// Base directory for isolated runtime environments
    runtime_envs_dir: PathBuf,
    /// Transport for distributing pack files to this worker
    pack_transport: Arc<dyn attune_common::pack_transport::PackFileTransport>,
    worker_token_provider: Arc<WorkerTokenProvider>,
    /// Semaphore to limit concurrent executions
    execution_semaphore: Arc<Semaphore>,
    /// Tracks in-flight execution tasks for graceful shutdown
    in_flight_tasks: Arc<Mutex<JoinSet<()>>>,
    /// Maps execution ID → CancellationToken for running processes.
    /// When a cancel request arrives, the token is triggered, causing
    /// the process executor to send SIGTERM → SIGKILL.
    cancel_tokens: Arc<Mutex<HashMap<i64, CancellationToken>>>,
    /// Tracks cancellation requests that arrived before the in-memory token
    /// for an execution had been registered.
    pending_cancellations: Arc<Mutex<HashSet<i64>>>,
    /// Controls whether this worker runs in full `Worker` mode (proactive
    /// environment setup, full version verification) or `Agent` mode (lazy
    /// setup, auto-detected runtimes).
    startup_mode: StartupMode,
}

impl WorkerService {
    /// Create a new worker service
    pub async fn new(config: Config) -> Result<Self> {
        info!("Initializing Worker Service");

        // Initialize database
        let db = Database::new(&config.database).await?;
        let pool = db.pool().clone();
        info!("Database connection established");

        // Initialize message queue connection
        let mq_url = config
            .message_queue
            .as_ref()
            .ok_or_else(|| Error::Internal("Message queue configuration is required".to_string()))?
            .url
            .as_str();

        let mq_connection = Connection::connect(mq_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to message queue: {}", e)))?;
        info!("Message queue connection established");

        // Setup common message queue infrastructure (exchanges and DLX)
        let mq_config = MqConfig::default();
        match mq_connection.setup_common_infrastructure(&mq_config).await {
            Ok(_) => info!("Common message queue infrastructure setup completed"),
            Err(e) => {
                warn!(
                    "Failed to setup common MQ infrastructure (may already exist): {}",
                    e
                );
            }
        }

        // Initialize message queue publisher
        let publisher = Publisher::new(
            &mq_connection,
            PublisherConfig {
                confirm_publish: true,
                timeout_secs: 30,
                exchange: "attune.executions".to_string(),
            },
        )
        .await
        .map_err(|e| Error::Internal(format!("Failed to create publisher: {}", e)))?;
        info!("Message queue publisher initialized");

        // Initialize worker registration
        let registration = Arc::new(RwLock::new(WorkerRegistration::new(pool.clone(), &config)));

        // Initialize artifact manager for execution stdout/stderr/result storage.
        // This must use the shared artifacts_dir so the API log streaming endpoints
        // and artifact download routes can see the same files the worker writes.
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Artifact storage root is a trusted deployment configuration value.
        let artifact_base_dir = std::path::PathBuf::from(&config.artifacts_dir);
        let artifact_manager = ArtifactManager::new(artifact_base_dir);
        artifact_manager.initialize().await?;

        // Initialize artifacts directory for file-backed artifact storage (shared volume).
        // Execution processes write artifact files here; the API serves them from the same path.
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Artifact storage root is a trusted deployment configuration value.
        let artifacts_dir = std::path::PathBuf::from(&config.artifacts_dir);
        if let Err(e) = attune_common::utils::create_shared_dir_all(&artifacts_dir).await {
            warn!(
                "Failed to create artifacts directory '{}': {}. File-backed artifacts may not work.",
                artifacts_dir.display(),
                e,
            );
        } else {
            info!(
                "Artifacts directory initialized at: {}",
                artifacts_dir.display()
            );
        }

        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Pack/runtime roots are trusted deployment configuration values.
        let packs_base_dir = std::path::PathBuf::from(&config.packs_base_dir);
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Pack/runtime roots are trusted deployment configuration values.
        let runtime_envs_dir = std::path::PathBuf::from(&config.runtime_envs_dir);
        cleanup_stale_pack_test_attempts(&packs_base_dir.join(".pack-test-attempts"));
        cleanup_stale_pack_test_attempts(&runtime_envs_dir.join(".pack-test-attempts"));

        // Determine which runtimes to register based on configuration
        // ATTUNE_WORKER_RUNTIMES env var filters which runtimes this worker handles.
        // If not set, all action runtimes from the database are loaded.
        let runtime_filter: Option<Vec<String>> =
            std::env::var("ATTUNE_WORKER_RUNTIMES").ok().map(|env_val| {
                info!(
                    "Filtering runtimes from ATTUNE_WORKER_RUNTIMES: {}",
                    env_val
                );
                env_val
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            });

        // Initialize runtime registry
        let mut runtime_registry = RuntimeRegistry::new();

        // Load runtimes from the database and create ProcessRuntime instances.
        // Each runtime row's `execution_config` JSONB drives how the ProcessRuntime
        // invokes interpreters, manages environments, and installs dependencies.
        // We skip runtimes with empty execution_config (e.g., core.native) since
        // they execute binaries directly and don't need a ProcessRuntime wrapper.
        match RuntimeRepository::list(&pool).await {
            Ok(db_runtimes) => {
                let executable_runtimes: Vec<_> = db_runtimes
                    .into_iter()
                    .filter(|r| {
                        let config = r.parsed_execution_config();
                        // A runtime is executable if it has a non-default interpreter
                        // (the default is "/bin/sh" from InterpreterConfig::default,
                        // but runtimes with no execution_config at all will have an
                        // empty JSON object that deserializes to defaults with no
                        // file_extension — those are not real process runtimes).
                        config.interpreter.file_extension.is_some()
                            || r.execution_config != serde_json::json!({})
                    })
                    .collect();

                info!(
                    "Found {} executable runtime(s) in database",
                    executable_runtimes.len()
                );

                for rt in executable_runtimes {
                    let rt_name = rt.name.to_lowercase();

                    // Apply filter if ATTUNE_WORKER_RUNTIMES is set.
                    // Uses alias-aware matching so that e.g. filter "node"
                    // matches DB runtime name "Node.js" (lowercased to "node.js").
                    if let Some(ref filter) = runtime_filter {
                        if !runtime_aliases_match_filter(&rt.aliases, filter) {
                            debug!(
                                "Skipping runtime '{}' (aliases {:?} not in ATTUNE_WORKER_RUNTIMES filter)",
                                rt_name, rt.aliases
                            );
                            continue;
                        }
                    }

                    let exec_config = rt.parsed_execution_config();
                    let process_runtime = ProcessRuntime::new(
                        rt_name.clone(),
                        exec_config,
                        packs_base_dir.clone(),
                        runtime_envs_dir.clone(),
                    );
                    runtime_registry.register(Box::new(process_runtime));
                    info!(
                        "Registered ProcessRuntime '{}' from database (ref: {})",
                        rt_name, rt.r#ref
                    );
                }
            }
            Err(e) => {
                warn!(
                    "Failed to load runtimes from database: {}. \
                     Falling back to built-in defaults.",
                    e
                );
            }
        }

        // If no runtimes were loaded from the DB, register built-in defaults
        if runtime_registry.list_runtimes().is_empty() {
            info!("No runtimes loaded from database, registering built-in defaults");

            // Shell runtime (always available) via generic ProcessRuntime
            let shell_runtime = ProcessRuntime::new(
                "shell".to_string(),
                attune_common::models::runtime::RuntimeExecutionConfig {
                    interpreter: attune_common::models::runtime::InterpreterConfig {
                        binary: "/bin/bash".to_string(),
                        args: vec![],
                        file_extension: Some(".sh".to_string()),
                    },
                    inline_execution: attune_common::models::runtime::InlineExecutionConfig {
                        strategy: attune_common::models::runtime::InlineExecutionStrategy::TempFile,
                        extension: Some(".sh".to_string()),
                        inject_shell_helpers: true,
                    },
                    environment: None,
                    dependencies: None,
                    env_vars: std::collections::HashMap::new(),
                },
                packs_base_dir.clone(),
                runtime_envs_dir.clone(),
            );
            runtime_registry.register(Box::new(shell_runtime));
            info!("Registered built-in shell ProcessRuntime");

            // Native runtime (for compiled binaries)
            runtime_registry.register(Box::new(NativeRuntime::new()));
            info!("Registered built-in Native runtime");

            // Local runtime as catch-all fallback
            let local_runtime = LocalRuntime::new();
            runtime_registry.register(Box::new(local_runtime));
            info!("Registered Local runtime (fallback)");
        }

        // Validate all registered runtimes
        runtime_registry
            .validate_all()
            .await
            .map_err(|e| Error::Internal(format!("Failed to validate runtimes: {}", e)))?;

        info!(
            "Successfully validated runtimes: {:?}",
            runtime_registry.list_runtimes()
        );

        // Initialize secret manager
        let encryption_key = config.security.encryption_key.clone();
        let secret_manager = SecretManager::new(pool.clone(), encryption_key)?;
        info!("Secret manager initialized");

        // Initialize action executor
        let max_stdout_bytes = config
            .worker
            .as_ref()
            .map(|w| w.max_stdout_bytes)
            .unwrap_or(10 * 1024 * 1024);
        let max_stderr_bytes = config
            .worker
            .as_ref()
            .map(|w| w.max_stderr_bytes)
            .unwrap_or(10 * 1024 * 1024);
        let execution_log_retention_policy = config
            .worker
            .as_ref()
            .map(|w| w.execution_log_retention_policy);
        let execution_log_retention_limit = config
            .worker
            .as_ref()
            .map(|w| w.execution_log_retention_limit);

        // Get API URL from environment or construct from server config
        let api_url = std::env::var("ATTUNE_API_URL")
            .unwrap_or_else(|_| format!("http://{}:{}", config.server.host, config.server.port));

        // Build JWT config for generating execution-scoped tokens
        let jwt_config = attune_common::auth::jwt::JwtConfig {
            secret: config
                .security
                .jwt_secret
                .clone()
                .unwrap_or_else(|| "insecure_default_secret_change_in_production".to_string()),
            access_token_expiration: config.security.jwt_access_expiration as i64,
            refresh_token_expiration: config.security.jwt_refresh_expiration as i64,
        };

        // Transport calls are made after registration, when this provider is
        // updated with the worker's database ID.
        let worker_token_provider = Arc::new(WorkerTokenProvider::new(
            1,
            "unregistered",
            jwt_config.clone(),
        ));

        // Initialize artifact file transport
        let transport: Arc<dyn attune_common::artifact_transport::ArtifactFileTransport> = {
            let transport_mode = &config.artifacts.transport;
            let transport =
                attune_common::artifact_transport::build_transport_with_worker_token_provider(
                    &config.artifacts_dir,
                    Some(&api_url),
                    Some(worker_token_provider.clone()),
                    transport_mode,
                );
            info!(
                "Artifact file transport initialized: mode={}",
                transport.transport_mode()
            );
            Arc::from(transport)
        };

        // Initialize pack file transport
        let pack_transport: Arc<dyn attune_common::pack_transport::PackFileTransport> = {
            let transport =
                attune_common::pack_transport::build_pack_transport_with_worker_token_provider(
                    &config.packs_base_dir,
                    Some(&api_url),
                    Some(worker_token_provider.clone()),
                );
            info!(
                "Pack file transport initialized: mode={}",
                transport.transport_mode()
            );
            Arc::from(transport)
        };

        let executor = Arc::new(ActionExecutor::new(
            pool.clone(),
            runtime_registry,
            artifact_manager,
            secret_manager,
            max_stdout_bytes,
            max_stderr_bytes,
            execution_log_retention_policy,
            execution_log_retention_limit,
            packs_base_dir.clone(),
            artifacts_dir,
            PathBuf::from(&config.runtime_envs_dir), // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- runtime_envs_dir is a trusted deployment configuration root, not request-derived input.
            api_url,
            jwt_config,
            transport,
        ));

        // Initialize heartbeat manager
        let heartbeat_interval = config
            .worker
            .as_ref()
            .map(|w| w.heartbeat_interval)
            .unwrap_or(30);
        let heartbeat = Arc::new(HeartbeatManager::new(
            registration.clone(),
            heartbeat_interval,
        ));

        // Capture the runtime filter for use in env setup
        let runtime_filter_for_service = runtime_filter.clone();

        // Read max concurrent tasks from config
        let max_concurrent_tasks = config
            .worker
            .as_ref()
            .map(|w| w.max_concurrent_tasks)
            .unwrap_or(10);
        info!(
            "Worker configured for max {} concurrent executions",
            max_concurrent_tasks
        );

        Ok(Self {
            config,
            db_pool: pool,
            registration,
            heartbeat,
            executor,
            mq_connection: Arc::new(mq_connection),
            publisher: Arc::new(publisher),
            consumer: None,
            consumer_handle: None,
            task_reaper_handle: None,
            pack_consumer: None,
            pack_consumer_handle: None,
            pack_test_consumer: None,
            pack_test_consumer_handle: None,
            cancel_consumer: None,
            cancel_consumer_handle: None,
            metadata_consumer_handle: None,
            worker_id: None,
            runtime_filter: runtime_filter_for_service,
            packs_base_dir,
            runtime_envs_dir,
            pack_transport,
            worker_token_provider,
            execution_semaphore: Arc::new(Semaphore::new(max_concurrent_tasks)),
            in_flight_tasks: Arc::new(Mutex::new(JoinSet::new())),
            cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
            pending_cancellations: Arc::new(Mutex::new(HashSet::new())),
            startup_mode: StartupMode::Worker,
        })
    }

    /// Set agent-detected runtimes for inclusion in worker registration.
    ///
    /// When the worker is started as `attune-agent`, the agent entrypoint
    /// auto-detects available interpreters and passes them here. During
    /// [`start()`](Self::start), the detection results are stored in the
    /// worker's capabilities as `detected_interpreters` (structured JSON
    /// with binary paths and versions) and the `agent_mode` flag is set.
    ///
    /// This method is a no-op for the standard `attune-worker` binary.
    pub fn with_detected_runtimes(mut self, runtimes: Vec<DetectedRuntime>) -> Self {
        self.startup_mode = StartupMode::Agent {
            detected_runtimes: runtimes,
        };
        self
    }

    /// Start the worker service
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting Worker Service");

        // Detect runtime capabilities and register worker
        let worker_id = {
            let mut reg = self.registration.write().await;
            reg.detect_capabilities(&self.config).await?;

            // If running as an agent, store the detected interpreter metadata
            // and set the agent_mode flag before registering.
            if let StartupMode::Agent {
                ref detected_runtimes,
            } = self.startup_mode
            {
                reg.set_detected_runtimes(detected_runtimes.clone());
                reg.set_agent_mode(true);
                info!(
                    "Agent mode: {} detected interpreter(s) will be stored in capabilities",
                    detected_runtimes.len()
                );
            }

            reg.register().await?
        };
        self.worker_id = Some(worker_id);
        self.worker_token_provider
            .set_worker_id(worker_id.to_string());

        info!("Worker registered with ID: {}", worker_id);

        // Setup worker-specific message queue infrastructure
        // (includes per-worker execution queue AND pack registration queue)
        let mq_config = MqConfig::default();
        self.mq_connection
            .setup_worker_infrastructure(worker_id, &mq_config)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to setup worker MQ infrastructure: {}", e))
            })?;
        info!("Worker-specific message queue infrastructure setup completed");

        // Sync pack files from API if not using a shared volume.
        // This must run before environment setup so that pack directories
        // (and their dependency manifests) are available locally.
        self.sync_all_packs_on_startup().await;

        match &self.startup_mode {
            StartupMode::Worker => {
                // Verify which runtime versions are available on this system.
                // This updates the `available` flag in the database so that
                // `select_best_version()` only considers genuinely present versions.
                self.verify_runtime_versions().await;

                // Proactively set up runtime environments for all registered packs.
                // This runs before we start consuming execution messages so that
                // environments are ready by the time the first execution arrives.
                // Now version-aware: creates per-version environments where needed.
                self.scan_and_setup_environments().await;
            }
            StartupMode::Agent { .. } => {
                // Skip proactive setup — will happen lazily on first execution
                info!(
                    "Agent mode: deferring environment setup and version verification to first use"
                );
            }
        }

        // Start heartbeat
        self.heartbeat.start().await?;

        // Start consuming execution messages
        self.start_execution_consumer().await?;

        // Start consuming pack registration events
        self.start_pack_consumer().await?;

        // Start consuming pack test requests
        self.start_pack_test_consumer().await?;

        // Start consuming cancel requests
        self.start_cancel_consumer().await?;

        // Start metadata invalidation consumer
        self.start_metadata_invalidation_consumer().await?;

        // Reap completed execution tasks continuously so finished JoinSet
        // entries do not accumulate for the lifetime of a busy worker.
        self.start_task_reaper();

        info!("Worker Service started successfully");

        Ok(())
    }

    /// Stop the worker service gracefully
    ///
    /// Shutdown order (mirrors sensor service pattern):
    /// 1. Deregister worker (mark inactive to stop receiving new work)
    /// 2. Stop heartbeat
    /// 3. Wait for in-flight tasks with timeout
    /// 4. Close MQ connection
    /// 5. Close DB connection
    ///
    /// Verify which runtime versions are available on this host/container.
    ///
    /// Runs each version's verification commands (from `distributions` JSONB)
    /// and updates the `available` flag in the database. This ensures that
    /// `select_best_version()` only considers versions whose interpreters
    /// are genuinely present.
    async fn verify_runtime_versions(&self) {
        let filter_refs: Option<Vec<String>> = self.runtime_filter.clone();
        let filter_slice: Option<&[String]> = filter_refs.as_deref();

        let result = version_verify::verify_all_runtime_versions(&self.db_pool, filter_slice).await;

        if !result.errors.is_empty() {
            warn!(
                "Runtime version verification completed with {} error(s): {:?}",
                result.errors.len(),
                result.errors,
            );
        } else {
            info!(
                "Runtime version verification complete: {} checked, \
                 {} available, {} unavailable",
                result.total_checked, result.available, result.unavailable,
            );
        }

        refresh_runtime_version_capabilities_for_registration(
            &self.db_pool,
            filter_slice,
            &self.registration,
        )
        .await;
    }

    /// Sync all registered packs from the API if using API-based pack transport.
    ///
    /// On startup, ensures that all packs from the database are available
    /// locally before runtime environment setup begins.
    async fn sync_all_packs_on_startup(&self) {
        if self.pack_transport.transport_mode() == "volume" {
            debug!("Pack transport is volume-based — skipping startup sync");
            return;
        }

        info!(
            "Syncing all packs via {} transport...",
            self.pack_transport.transport_mode()
        );

        let packs = match attune_common::repositories::PackRepository::list(&self.db_pool).await {
            Ok(packs) => packs,
            Err(e) => {
                warn!("Failed to list packs for startup sync: {}", e);
                return;
            }
        };

        let mut synced = 0;
        let mut skipped = 0;
        let mut errors = 0;

        for pack in &packs {
            if self.pack_transport.is_pack_local(&pack.r#ref).await {
                skipped += 1;
                continue;
            }

            match self.pack_transport.sync_pack(&pack.r#ref).await {
                Ok(()) => {
                    synced += 1;
                    debug!("Synced pack '{}'", pack.r#ref);
                }
                Err(e) => {
                    errors += 1;
                    warn!("Failed to sync pack '{}': {}", pack.r#ref, e);
                }
            }
        }

        info!(
            "Pack startup sync complete: {} synced, {} already local, {} errors (of {} total)",
            synced,
            skipped,
            errors,
            packs.len()
        );
    }

    /// Scan all registered packs and create missing runtime environments.
    async fn scan_and_setup_environments(&self) {
        let Some(worker_id) = self.worker_id else {
            warn!("Skipping environment startup scan because worker is not registered yet");
            return;
        };
        let filter_refs: Option<Vec<String>> = self.runtime_filter.clone();
        let filter_slice: Option<&[String]> = filter_refs.as_deref();

        let result = env_setup::scan_and_setup_all_environments(
            &self.db_pool,
            worker_id,
            filter_slice,
            &self.packs_base_dir,
            &self.runtime_envs_dir,
        )
        .await;

        if !result.errors.is_empty() {
            warn!(
                "Environment startup scan completed with {} error(s): {:?}",
                result.errors.len(),
                result.errors,
            );
        } else {
            info!(
                "Environment startup scan completed: {} pack(s) scanned, \
                 {} environment(s) ensured, {} skipped",
                result.packs_scanned, result.environments_created, result.environments_skipped,
            );
        }
    }

    /// Start consuming pack.registered events from the per-worker packs queue.
    async fn start_pack_consumer(&mut self) -> Result<()> {
        let worker_id = self
            .worker_id
            .ok_or_else(|| Error::Internal("Worker not registered".to_string()))?;

        let queue_name = format!("worker.{}.packs", worker_id);
        info!(
            "Starting pack registration consumer for queue: {}",
            queue_name
        );

        let consumer = Arc::new(
            Consumer::new(
                &self.mq_connection,
                ConsumerConfig {
                    queue: queue_name.clone(),
                    tag: format!("worker-{}-packs", worker_id),
                    prefetch_count: 5,
                    auto_ack: false,
                    exclusive: false,
                },
            )
            .await
            .map_err(|e| Error::Internal(format!("Failed to create pack consumer: {}", e)))?,
        );

        let db_pool = self.db_pool.clone();
        let consumer_for_task = consumer.clone();
        let queue_name_for_log = queue_name.clone();
        let runtime_filter = self.runtime_filter.clone();
        let packs_base_dir = self.packs_base_dir.clone();
        let runtime_envs_dir = self.runtime_envs_dir.clone();
        let registration = self.registration.clone();
        let pack_transport = self.pack_transport.clone();

        let handle = tokio::spawn(async move {
            info!(
                "Pack consumer loop started for queue '{}'",
                queue_name_for_log
            );
            let result = consumer_for_task
                .consume_with_handler(move |envelope: MessageEnvelope<serde_json::Value>| {
                    let db_pool = db_pool.clone();
                    let runtime_filter = runtime_filter.clone();
                    let packs_base_dir = packs_base_dir.clone();
                    let runtime_envs_dir = runtime_envs_dir.clone();
                    let registration = registration.clone();
                    let pack_transport = pack_transport.clone();

                    async move {
                        match envelope.message_type {
                            MessageType::PackRegistered => {
                                let payload: PackRegisteredPayload =
                                    serde_json::from_value(envelope.payload).map_err(|e| {
                                        MqError::Deserialization(format!(
                                            "Failed to parse PackRegistered payload: {}",
                                            e
                                        ))
                                    })?;
                                validate_mq_pack_ref(&payload.pack_ref, "PackRegistered")?;

                                info!(
                                    "Received pack.registered event for pack '{}' (version {})",
                                    payload.pack_ref, payload.version,
                                );

                                // Sync pack files from API if not on a shared volume
                                if pack_transport.transport_mode() != "volume" {
                                    match pack_transport.sync_pack(&payload.pack_ref).await {
                                        Ok(()) => info!(
                                            "Pack '{}' synced via {} transport",
                                            payload.pack_ref,
                                            pack_transport.transport_mode(),
                                        ),
                                        Err(e) => warn!(
                                            "Failed to sync pack '{}': {}",
                                            payload.pack_ref, e,
                                        ),
                                    }
                                }

                                let filter_slice: Option<Vec<String>> = runtime_filter;
                                let filter_ref: Option<&[String]> = filter_slice.as_deref();

                                let filtered_runtime_names = filter_runtime_names_for_worker(
                                    &payload.runtime_names,
                                    filter_ref,
                                );

                                if !filtered_runtime_names.is_empty() {
                                    let verify_result =
                                        version_verify::verify_all_runtime_versions(
                                            &db_pool,
                                            Some(filtered_runtime_names.as_slice()),
                                        )
                                        .await;

                                    if !verify_result.errors.is_empty() {
                                        warn!(
                                            "Pack '{}' runtime verification had {} error(s): {:?}",
                                            payload.pack_ref,
                                            verify_result.errors.len(),
                                            verify_result.errors,
                                        );
                                    }

                                    refresh_runtime_version_capabilities_for_registration(
                                        &db_pool,
                                        filter_ref,
                                        &registration,
                                    )
                                    .await;
                                }

                                let pack_result =
                                    env_setup::setup_environments_for_registered_pack(
                                        &db_pool,
                                        worker_id,
                                        &payload,
                                        filter_ref,
                                        &packs_base_dir,
                                        &runtime_envs_dir,
                                    )
                                    .await;

                                if !pack_result.errors.is_empty() {
                                    warn!(
                                        "Pack '{}' environment setup had {} error(s): {:?}",
                                        pack_result.pack_ref,
                                        pack_result.errors.len(),
                                        pack_result.errors,
                                    );
                                } else if !pack_result.environments_created.is_empty() {
                                    info!(
                                        "Pack '{}' environments set up: {:?}",
                                        pack_result.pack_ref, pack_result.environments_created,
                                    );
                                }
                            }
                            MessageType::PackDeleted => {
                                let payload: PackDeletedPayload =
                                    serde_json::from_value(envelope.payload).map_err(|e| {
                                        MqError::Deserialization(format!(
                                            "Failed to parse PackDeleted payload: {}",
                                            e
                                        ))
                                    })?;
                                validate_mq_pack_ref(&payload.pack_ref, "PackDeleted")?;

                                info!(
                                    "Received pack.deleted event for pack '{}'",
                                    payload.pack_ref,
                                );

                                // Remove local pack files
                                match pack_transport.remove_pack(&payload.pack_ref).await {
                                    Ok(()) => info!("Pack '{}' removed locally", payload.pack_ref,),
                                    Err(e) => warn!(
                                        "Failed to remove pack '{}' locally: {}",
                                        payload.pack_ref, e,
                                    ),
                                }

                                // Clean up runtime environments for this pack
                                let pack_env_dir = runtime_envs_dir.join(&payload.pack_ref);
                                if pack_env_dir.exists() {
                                    match tokio::fs::remove_dir_all(&pack_env_dir).await {
                                        Ok(()) => info!(
                                            "Cleaned up runtime environments for pack '{}'",
                                            payload.pack_ref,
                                        ),
                                        Err(e) => warn!(
                                            "Failed to clean up runtime envs for pack '{}': {}",
                                            payload.pack_ref, e,
                                        ),
                                    }
                                }
                            }
                            other => {
                                warn!(
                                    "Unexpected message type {:?} on pack queue — skipping",
                                    other,
                                );
                            }
                        }

                        Ok(())
                    }
                })
                .await;

            match result {
                Ok(()) => info!(
                    "Pack consumer loop for queue '{}' ended",
                    queue_name_for_log
                ),
                Err(e) => error!(
                    "Pack consumer loop for queue '{}' failed: {}",
                    queue_name_for_log, e
                ),
            }
        });

        self.pack_consumer = Some(consumer);
        self.pack_consumer_handle = Some(handle);

        info!("Pack registration consumer initialized");

        Ok(())
    }

    /// Start consuming pack test requests from the executor's dispatch queue.
    ///
    /// Pack test suites run on the worker (which has the required interpreters
    /// and runtime environments) rather than on the API container. Each handled
    /// request runs the pack's tests, persists the result to `pack_test_execution`,
    /// and records the outcome on the owning `pack_install` record.
    async fn start_pack_test_consumer(&mut self) -> Result<()> {
        let worker_id = self
            .worker_id
            .ok_or_else(|| Error::Internal("Worker not registered".to_string()))?;

        let queue_name = format!("worker.{}.packtests", worker_id);
        info!("Starting pack test consumer for queue: {}", queue_name);

        let consumer = Arc::new(
            Consumer::new(
                &self.mq_connection,
                ConsumerConfig {
                    queue: queue_name.clone(),
                    tag: format!("worker-{}-packtests", worker_id),
                    prefetch_count: 1,
                    auto_ack: false,
                    exclusive: false,
                },
            )
            .await
            .map_err(|e| Error::Internal(format!("Failed to create pack test consumer: {}", e)))?,
        );

        let db_pool = self.db_pool.clone();
        let packs_base_dir = self.packs_base_dir.clone();
        let runtime_envs_dir = self.runtime_envs_dir.clone();
        let pack_transport = self.pack_transport.clone();

        let consumer_for_task = consumer.clone();
        let handle = tokio::spawn(async move {
            info!("Pack test consumer loop started for queue '{}'", queue_name);
            let result = consumer_for_task
                .clone()
                .consume_with_handler(
                    move |envelope: MessageEnvelope<
                        attune_common::mq::PackTestRequestedPayload,
                    >| {
                        let db_pool = db_pool.clone();
                        let packs_base_dir = packs_base_dir.clone();
                        let runtime_envs_dir = runtime_envs_dir.clone();
                        let pack_transport = pack_transport.clone();

                        async move {
                            let payload = envelope.payload;
                            if let Err(e) = handle_pack_test(
                                &db_pool,
                                worker_id,
                                packs_base_dir.as_path(),
                                runtime_envs_dir.as_path(),
                                pack_transport.as_ref(),
                                &payload,
                            )
                            .await
                            {
                                error!(
                                    "Failed to run pack tests for install {} ({}): {}",
                                    payload.pack_install_id, payload.pack_ref, e
                                );
                                // Record the failure so the API's install finalizer can react.
                                if let Err(mark_err) =
                                    mark_pack_install_failed(&db_pool, &payload, &e.to_string())
                                        .await
                                {
                                    error!(
                                        "Failed to record pack install {} failure: {}",
                                        payload.pack_install_id, mark_err
                                    );
                                }
                                return Err(format!("Failed to run pack tests: {}", e).into());
                            }
                            Ok(())
                        }
                    },
                )
                .await;

            match result {
                Ok(()) => info!("Pack test consumer loop for queue '{}' ended", queue_name),
                Err(e) => error!(
                    "Pack test consumer loop for queue '{}' failed: {}",
                    queue_name, e
                ),
            }
        });

        self.pack_test_consumer = Some(consumer);
        self.pack_test_consumer_handle = Some(handle);

        info!("Pack test consumer initialized");

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping Worker Service - initiating graceful shutdown");

        // 1. Mark worker as inactive first to stop receiving new tasks
        // Use if-let instead of ? so shutdown continues even if DB call fails
        {
            let reg = self.registration.read().await;
            info!("Marking worker as inactive to stop receiving new tasks");
            if let Err(e) = reg.deregister().await {
                error!("Failed to deregister worker: {}", e);
            }
        }

        // 2. Stop heartbeat
        info!("Stopping heartbeat updates");
        self.heartbeat.stop().await;

        // 3. Wait for in-flight tasks to complete (with timeout)
        let shutdown_timeout = self
            .config
            .worker
            .as_ref()
            .and_then(|w| w.shutdown_timeout)
            .unwrap_or(30); // Default: 30 seconds

        info!(
            "Waiting up to {} seconds for in-flight tasks to complete",
            shutdown_timeout
        );

        let timeout_duration = Duration::from_secs(shutdown_timeout);
        match tokio::time::timeout(timeout_duration, self.wait_for_in_flight_tasks()).await {
            Ok(_) => info!("All in-flight tasks completed"),
            Err(_) => warn!("Shutdown timeout reached - some tasks may have been interrupted"),
        }

        // 4. Abort consumer tasks and close message queue connection
        if let Some(handle) = self.consumer_handle.take() {
            info!("Stopping execution consumer task...");
            handle.abort();
            // Wait briefly for the task to finish
            let _ = handle.await;
        }

        if let Some(handle) = self.task_reaper_handle.take() {
            info!("Stopping in-flight task reaper...");
            handle.abort();
            let _ = handle.await;
        }

        if let Some(handle) = self.pack_consumer_handle.take() {
            info!("Stopping pack consumer task...");
            handle.abort();
            let _ = handle.await;
        }

        if let Some(handle) = self.pack_test_consumer_handle.take() {
            info!("Stopping pack test consumer task...");
            handle.abort();
            let _ = handle.await;
        }

        if let Some(handle) = self.cancel_consumer_handle.take() {
            info!("Stopping cancel consumer task...");
            handle.abort();
            let _ = handle.await;
        }

        if let Some(handle) = self.metadata_consumer_handle.take() {
            info!("Stopping metadata invalidation consumer task...");
            handle.abort();
            let _ = handle.await;
        }

        info!("Closing message queue connection...");
        if let Err(e) = self.mq_connection.close().await {
            warn!("Error closing message queue: {}", e);
        }

        // 5. Close database connection
        info!("Closing database connection...");
        self.db_pool.close().await;

        info!("Worker Service stopped");

        Ok(())
    }

    /// Wait for in-flight tasks to complete
    async fn wait_for_in_flight_tasks(&self) {
        loop {
            let remaining = self.reap_completed_in_flight_tasks().await;

            if remaining == 0 {
                info!("All in-flight execution tasks have completed");
                break;
            }

            info!(
                "Waiting for {} in-flight execution task(s) to complete...",
                remaining
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn reap_completed_in_flight_tasks(&self) -> usize {
        let mut tasks = self.in_flight_tasks.lock().await;
        let drained = Self::drain_completed_in_flight_tasks(&mut tasks);
        if drained > 0 {
            debug!("Reaped {} completed in-flight execution task(s)", drained);
        }
        tasks.len()
    }

    fn drain_completed_in_flight_tasks(tasks: &mut JoinSet<()>) -> usize {
        let mut drained = 0;
        while let Some(join_result) = tasks.try_join_next() {
            drained += 1;
            if let Err(e) = join_result {
                warn!("In-flight execution task ended with join error: {}", e);
            }
        }
        drained
    }

    fn start_task_reaper(&mut self) {
        let in_flight = self.in_flight_tasks.clone();
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                let drained = {
                    let mut tasks = in_flight.lock().await;
                    Self::drain_completed_in_flight_tasks(&mut tasks)
                };

                if drained > 0 {
                    debug!("Reaped {} completed in-flight execution task(s)", drained);
                }
            }
        });

        self.task_reaper_handle = Some(handle);
    }

    /// Start consuming execution.scheduled messages
    ///
    /// Spawns the consumer loop as a background task so that `start()` returns
    /// immediately, allowing the caller to set up signal handlers.
    ///
    /// Executions are spawned as concurrent background tasks, limited by the
    /// `execution_semaphore`. The consumer blocks when the concurrency limit is
    /// reached, providing natural backpressure via RabbitMQ.
    async fn start_execution_consumer(&mut self) -> Result<()> {
        let worker_id = self
            .worker_id
            .ok_or_else(|| Error::Internal("Worker not registered".to_string()))?;

        // Queue name for this worker (already created in setup_worker_infrastructure)
        let queue_name = format!("worker.{}.executions", worker_id);

        // Set prefetch slightly above max concurrent tasks to keep the pipeline filled
        let max_concurrent = self
            .config
            .worker
            .as_ref()
            .map(|w| w.max_concurrent_tasks)
            .unwrap_or(10);
        let prefetch_count = (max_concurrent as u16).saturating_add(2);

        info!(
            "Starting consumer for worker queue: {} (prefetch: {}, max_concurrent: {})",
            queue_name, prefetch_count, max_concurrent
        );

        // Create consumer
        let consumer = Arc::new(
            Consumer::new(
                &self.mq_connection,
                ConsumerConfig {
                    queue: queue_name.clone(),
                    tag: format!("worker-{}", worker_id),
                    prefetch_count,
                    auto_ack: false,
                    exclusive: false,
                },
            )
            .await
            .map_err(|e| Error::Internal(format!("Failed to create consumer: {}", e)))?,
        );

        info!("Consumer created for queue: {}", queue_name);

        // Clone Arc references for the spawned task
        let executor = self.executor.clone();
        let publisher = self.publisher.clone();
        let db_pool = self.db_pool.clone();
        let consumer_for_task = consumer.clone();
        let queue_name_for_log = queue_name.clone();
        let semaphore = self.execution_semaphore.clone();
        let in_flight = self.in_flight_tasks.clone();
        let cancel_tokens = self.cancel_tokens.clone();
        let pending_cancellations = self.pending_cancellations.clone();

        // Spawn the consumer loop as a background task so start() can return
        let handle = tokio::spawn(async move {
            info!("Consumer loop started for queue '{}'", queue_name_for_log);
            let result = consumer_for_task
                .consume_with_handler(
                    move |envelope: MessageEnvelope<ExecutionScheduledPayload>| {
                        let executor = executor.clone();
                        let publisher = publisher.clone();
                        let db_pool = db_pool.clone();
                        let semaphore = semaphore.clone();
                        let in_flight = in_flight.clone();
                        let cancel_tokens = cancel_tokens.clone();
                        let pending_cancellations = pending_cancellations.clone();

                        async move {
                            let execution_id = envelope.payload.execution_id;

                            // Acquire a concurrency permit. This blocks if we're at the
                            // max concurrent execution limit, providing natural backpressure:
                            // the message won't be acked until we can actually start working,
                            // so RabbitMQ will stop delivering once prefetch is exhausted.
                            let permit = semaphore.clone().acquire_owned().await.map_err(|_| {
                                attune_common::mq::error::MqError::Channel(
                                    "Execution semaphore closed".to_string(),
                                )
                            })?;

                            info!(
                                "Acquired execution permit for execution {} ({} permits remaining)",
                                execution_id,
                                semaphore.available_permits()
                            );

                            // Create a cancellation token for this execution
                            let cancel_token = CancellationToken::new();
                            {
                                let mut tokens = cancel_tokens.lock().await;
                                tokens.insert(execution_id, cancel_token.clone());
                            }
                            {
                                let pending = pending_cancellations.lock().await;
                                if pending.contains(&execution_id) {
                                    info!(
                                        "Execution {} already had a pending cancel request; cancelling immediately",
                                        execution_id
                                    );
                                    cancel_token.cancel();
                                }
                            }

                            // Spawn the actual execution as a background task so this
                            // handler returns immediately, acking the message and freeing
                            // the consumer loop to process the next delivery.
                            let mut tasks = in_flight.lock().await;
                            Self::drain_completed_in_flight_tasks(&mut tasks);
                            tasks.spawn(async move {
                                // The permit is moved into this task and will be released
                                // when the task completes (on drop).
                                let _permit = permit;

                                if let Err(e) = Self::handle_execution_scheduled(
                                    executor,
                                    publisher,
                                    db_pool,
                                    envelope,
                                    cancel_token,
                                )
                                .await
                                {
                                    error!("Execution {} handler error: {}", execution_id, e);
                                }

                                // Remove the cancel token now that execution is done
                                let mut tokens = cancel_tokens.lock().await;
                                tokens.remove(&execution_id);
                                let mut pending = pending_cancellations.lock().await;
                                pending.remove(&execution_id);
                            });

                            Ok(())
                        }
                    },
                )
                .await;

            match result {
                Ok(()) => info!("Consumer loop for queue '{}' ended", queue_name_for_log),
                Err(e) => error!(
                    "Consumer loop for queue '{}' failed: {}",
                    queue_name_for_log, e
                ),
            }
        });

        // Store consumer reference and task handle
        self.consumer = Some(consumer);
        self.consumer_handle = Some(handle);

        info!("Message queue consumer initialized");

        Ok(())
    }

    /// Handle execution.scheduled message
    async fn handle_execution_scheduled(
        executor: Arc<ActionExecutor>,
        publisher: Arc<Publisher>,
        db_pool: PgPool,
        envelope: MessageEnvelope<ExecutionScheduledPayload>,
        cancel_token: CancellationToken,
    ) -> Result<()> {
        let execution_id = envelope.payload.execution_id;

        info!(
            "Processing execution.scheduled for execution: {}",
            execution_id
        );

        // Check if the execution was already cancelled before we started
        // (e.g. pre-running cancellation via the API).
        {
            if let Ok(Some(exec)) = ExecutionRepository::find_by_id(&db_pool, execution_id).await {
                if matches!(
                    exec.status,
                    ExecutionStatus::Cancelled | ExecutionStatus::Canceling
                ) {
                    info!(
                        "Execution {} already in {:?} state, skipping",
                        execution_id, exec.status
                    );
                    // If it was Canceling, finalize to Cancelled
                    if exec.status == ExecutionStatus::Canceling {
                        let _ = Self::publish_status_update(
                            &db_pool,
                            &publisher,
                            execution_id,
                            ExecutionStatus::Cancelled,
                            None,
                            Some("Cancelled before execution started".to_string()),
                        )
                        .await;
                        let _ = Self::publish_completion_notification(
                            &db_pool,
                            &publisher,
                            execution_id,
                            ExecutionStatus::Cancelled,
                        )
                        .await;
                    }
                    return Ok(());
                }
            }
        }

        // Publish status: running
        if let Err(e) = Self::publish_status_update(
            &db_pool,
            &publisher,
            execution_id,
            ExecutionStatus::Running,
            None,
            None,
        )
        .await
        {
            error!("Failed to publish running status: {}", e);
            // Continue anyway - we'll update the database directly
        }

        // Execute the action (with cancellation support)
        match executor
            .execute_with_cancel(execution_id, cancel_token.clone())
            .await
        {
            Ok(result) => {
                // Check if this was a cancellation
                let was_cancelled = cancel_token.is_cancelled()
                    || result
                        .error
                        .as_deref()
                        .is_some_and(|e| e.contains("cancelled"));

                if was_cancelled {
                    info!(
                        "Execution {} was cancelled in {}ms",
                        execution_id, result.duration_ms
                    );

                    // Publish status: cancelled
                    if let Err(e) = Self::publish_status_update(
                        &db_pool,
                        &publisher,
                        execution_id,
                        ExecutionStatus::Cancelled,
                        None,
                        Some("Cancelled by user".to_string()),
                    )
                    .await
                    {
                        error!("Failed to publish cancelled status: {}", e);
                    }

                    // Publish completion notification for queue management
                    if let Err(e) = Self::publish_completion_notification(
                        &db_pool,
                        &publisher,
                        execution_id,
                        ExecutionStatus::Cancelled,
                    )
                    .await
                    {
                        error!(
                            "Failed to publish completion notification for cancelled execution {}: {}",
                            execution_id, e
                        );
                    }
                } else {
                    info!(
                        "Execution {} completed successfully in {}ms",
                        execution_id, result.duration_ms
                    );

                    // Publish status: completed
                    if let Err(e) = Self::publish_status_update(
                        &db_pool,
                        &publisher,
                        execution_id,
                        ExecutionStatus::Completed,
                        result.result.clone(),
                        None,
                    )
                    .await
                    {
                        error!("Failed to publish success status: {}", e);
                    }

                    // Publish completion notification for queue management
                    if let Err(e) = Self::publish_completion_notification(
                        &db_pool,
                        &publisher,
                        execution_id,
                        ExecutionStatus::Completed,
                    )
                    .await
                    {
                        error!(
                            "Failed to publish completion notification for execution {}: {}",
                            execution_id, e
                        );
                        // Continue - this is important for queue management but not fatal
                    }
                }
            }
            Err(e) => {
                error!("Execution {} failed: {}", execution_id, e);

                // Publish status: failed
                if let Err(e) = Self::publish_status_update(
                    &db_pool,
                    &publisher,
                    execution_id,
                    ExecutionStatus::Failed,
                    None,
                    Some(e.to_string()),
                )
                .await
                {
                    error!("Failed to publish failure status: {}", e);
                }

                // Publish completion notification for queue management
                if let Err(e) = Self::publish_completion_notification(
                    &db_pool,
                    &publisher,
                    execution_id,
                    ExecutionStatus::Failed,
                )
                .await
                {
                    error!(
                        "Failed to publish completion notification for execution {}: {}",
                        execution_id, e
                    );
                    // Continue - this is important for queue management but not fatal
                }
            }
        }

        Ok(())
    }

    /// Start consuming execution cancel requests from the per-worker cancel queue.
    async fn start_cancel_consumer(&mut self) -> Result<()> {
        let worker_id = self
            .worker_id
            .ok_or_else(|| Error::Internal("Worker not registered".to_string()))?;

        let queue_name = format!("worker.{}.cancel", worker_id);

        info!("Starting cancel consumer for queue: {}", queue_name);

        let consumer = Arc::new(
            Consumer::new(
                &self.mq_connection,
                ConsumerConfig {
                    queue: queue_name.clone(),
                    tag: format!("worker-{}-cancel", worker_id),
                    prefetch_count: 10,
                    auto_ack: false,
                    exclusive: false,
                },
            )
            .await
            .map_err(|e| Error::Internal(format!("Failed to create cancel consumer: {}", e)))?,
        );

        let consumer_for_task = consumer.clone();
        let cancel_tokens = self.cancel_tokens.clone();
        let pending_cancellations = self.pending_cancellations.clone();
        let queue_name_for_log = queue_name.clone();

        let handle = tokio::spawn(async move {
            info!(
                "Cancel consumer loop started for queue '{}'",
                queue_name_for_log
            );
            let result = consumer_for_task
                .consume_with_handler(
                    move |envelope: MessageEnvelope<ExecutionCancelRequestedPayload>| {
                        let cancel_tokens = cancel_tokens.clone();
                        let pending_cancellations = pending_cancellations.clone();

                        async move {
                            let execution_id = envelope.payload.execution_id;
                            info!("Received cancel request for execution {}", execution_id);

                            {
                                let mut pending = pending_cancellations.lock().await;
                                pending.insert(execution_id);
                            }

                            let tokens = cancel_tokens.lock().await;
                            if let Some(token) = tokens.get(&execution_id) {
                                info!("Triggering cancellation for execution {}", execution_id);
                                token.cancel();
                            } else {
                                warn!(
                                    "No cancel token found for execution {} \
                                     (may have already completed or not yet started)",
                                    execution_id
                                );
                            }

                            Ok(())
                        }
                    },
                )
                .await;

            match result {
                Ok(()) => info!(
                    "Cancel consumer loop for queue '{}' ended",
                    queue_name_for_log
                ),
                Err(e) => error!(
                    "Cancel consumer loop for queue '{}' failed: {}",
                    queue_name_for_log, e
                ),
            }
        });

        self.cancel_consumer = Some(consumer);
        self.cancel_consumer_handle = Some(handle);

        info!("Cancel consumer initialized for queue: {}", queue_name);

        Ok(())
    }

    async fn start_metadata_invalidation_consumer(&mut self) -> Result<()> {
        let worker_id = self
            .worker_id
            .ok_or_else(|| Error::Internal("Worker not registered".to_string()))?;

        info!(
            "Starting metadata invalidation consumer for worker {}",
            worker_id
        );

        let mq_url = self
            .config
            .message_queue
            .as_ref()
            .ok_or_else(|| Error::Internal("Message queue configuration is required".to_string()))?
            .url
            .clone();
        let executor = self.executor.clone();
        let handle = tokio::spawn(async move {
            loop {
                match Connection::connect(&mq_url).await {
                    Ok(connection) => {
                        match connection
                            .create_ephemeral_topic_consumer(
                                "attune.metadata",
                                &[
                                    routing_keys::METADATA_ACTION_CHANGED,
                                    routing_keys::METADATA_RUNTIME_CHANGED,
                                    routing_keys::METADATA_PACK_CHANGED,
                                ],
                                &format!("worker-{}-metadata", worker_id),
                                32,
                            )
                            .await
                        {
                            Ok(consumer) => {
                                let executor = executor.clone();
                                let result = consumer
                                    .consume_once_with_handler(
                                        move |envelope: MessageEnvelope<serde_json::Value>| {
                                            let executor = executor.clone();
                                            async move {
                                                handle_metadata_invalidation(&executor, envelope)
                                                    .await
                                            }
                                        },
                                    )
                                    .await;

                                if let Err(error) = result {
                                    warn!(
                                        "Metadata invalidation consumer ended for worker {}: {}",
                                        worker_id, error
                                    );
                                }
                            }
                            Err(error) => warn!(
                                "Failed to create metadata invalidation consumer for worker {}: {}",
                                worker_id, error
                            ),
                        }
                    }
                    Err(error) => warn!(
                        "Failed to connect MQ for metadata invalidation consumer on worker {}: {}",
                        worker_id, error
                    ),
                }

                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });

        self.metadata_consumer_handle = Some(handle);

        info!(
            "Metadata invalidation consumer initialized for worker {}",
            worker_id
        );
        Ok(())
    }

    /// Publish execution status update
    async fn publish_status_update(
        db_pool: &PgPool,
        publisher: &Publisher,
        execution_id: i64,
        status: ExecutionStatus,
        _result: Option<serde_json::Value>,
        _error: Option<String>,
    ) -> Result<()> {
        // Fetch execution to get action_ref and previous status
        let execution = ExecutionRepository::find_by_id(db_pool, execution_id)
            .await?
            .ok_or_else(|| {
                Error::Internal(format!(
                    "Execution {} not found for status update",
                    execution_id
                ))
            })?;

        let new_status_str = match status {
            ExecutionStatus::Running => "running",
            ExecutionStatus::Completed => "completed",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Canceling => "canceling",
            ExecutionStatus::Cancelled => "cancelled",
            ExecutionStatus::Timeout => "timeout",
            _ => "unknown",
        };

        let previous_status_str = format!("{:?}", execution.status).to_lowercase();

        let payload = ExecutionStatusChangedPayload {
            execution_id,
            action_ref: execution.action_ref,
            previous_status: previous_status_str,
            new_status: new_status_str.to_string(),
            changed_at: Utc::now(),
        };

        let message_type = MessageType::ExecutionStatusChanged;

        let envelope = MessageEnvelope::new(message_type, payload).with_source("worker");

        publisher
            .publish_envelope(&envelope)
            .await
            .map_err(|e| Error::Internal(format!("Failed to publish status update: {}", e)))?;

        Ok(())
    }

    /// Publish execution completion notification for queue management
    async fn publish_completion_notification(
        db_pool: &PgPool,
        publisher: &Publisher,
        execution_id: i64,
        final_status: ExecutionStatus,
    ) -> Result<()> {
        // Fetch execution to get action_id and other required fields
        let execution = ExecutionRepository::find_by_id(db_pool, execution_id)
            .await?
            .ok_or_else(|| {
                Error::Internal(format!(
                    "Execution {} not found after completion",
                    execution_id
                ))
            })?;

        // Extract action_id - it should always be present for valid executions
        let action_id = execution.action.ok_or_else(|| {
            Error::Internal(format!(
                "Execution {} has no associated action",
                execution_id
            ))
        })?;

        info!(
            "Publishing completion notification for execution {} (action_id: {})",
            execution_id, action_id
        );

        let payload = ExecutionCompletedPayload {
            execution_id: execution.id,
            action_id,
            action_ref: execution.action_ref.clone(),
            status: format!("{:?}", final_status),
            result: execution.result.clone(),
            completed_at: Utc::now(),
        };

        let envelope =
            MessageEnvelope::new(MessageType::ExecutionCompleted, payload).with_source("worker");

        publisher.publish_envelope(&envelope).await.map_err(|e| {
            Error::Internal(format!("Failed to publish completion notification: {}", e))
        })?;

        info!(
            "Completion notification published for execution {}",
            execution_id
        );

        Ok(())
    }
}

fn validate_mq_pack_ref(pack_ref: &str, message_type: &str) -> std::result::Result<(), MqError> {
    RefValidator::validate_pack_ref(pack_ref).map_err(|error| {
        MqError::InvalidMessage(format!(
            "{message_type} payload contains invalid pack_ref '{pack_ref}': {error}"
        ))
    })
}

async fn handle_metadata_invalidation(
    executor: &ActionExecutor,
    envelope: MessageEnvelope<serde_json::Value>,
) -> std::result::Result<(), MqError> {
    match envelope.message_type {
        MessageType::ActionChanged => {
            let payload: ActionChangedPayload =
                serde_json::from_value(envelope.payload).map_err(|error| {
                    MqError::Deserialization(format!(
                        "Failed to parse ActionChanged payload: {}",
                        error
                    ))
                })?;
            validate_mq_pack_ref(&payload.pack_ref, "ActionChanged")?;
            debug!(
                entity = "action",
                operation = %payload.operation,
                action_id = payload.action_id,
                action_ref = %payload.action_ref,
                "Received metadata invalidation event"
            );
            executor
                .invalidate_action_cache(Some(payload.action_id), Some(payload.action_ref.as_str()))
                .await;
        }
        MessageType::RuntimeChanged => {
            let payload: RuntimeChangedPayload =
                serde_json::from_value(envelope.payload).map_err(|error| {
                    MqError::Deserialization(format!(
                        "Failed to parse RuntimeChanged payload: {}",
                        error
                    ))
                })?;
            if let Some(pack_ref) = payload.pack_ref.as_deref() {
                validate_mq_pack_ref(pack_ref, "RuntimeChanged")?;
            }
            debug!(
                entity = "runtime",
                operation = %payload.operation,
                runtime_id = payload.runtime_id,
                runtime_ref = %payload.runtime_ref,
                "Received metadata invalidation event"
            );
            executor
                .invalidate_runtime_cache(
                    Some(payload.runtime_id),
                    Some(payload.runtime_ref.as_str()),
                )
                .await;
        }
        MessageType::PackChanged => {
            let payload: PackChangedPayload =
                serde_json::from_value(envelope.payload).map_err(|error| {
                    MqError::Deserialization(format!(
                        "Failed to parse PackChanged payload: {}",
                        error
                    ))
                })?;
            validate_mq_pack_ref(&payload.pack_ref, "PackChanged")?;
            debug!(
                entity = "pack",
                operation = %payload.operation,
                pack_id = payload.pack_id,
                pack_ref = %payload.pack_ref,
                "Received metadata invalidation event"
            );
            env_setup::invalidate_pack_metadata_cache(Some(payload.pack_ref.as_str())).await;
        }
        _ => {}
    }
    Ok(())
}

struct PackTestCandidateCleanup {
    files: Option<PathBuf>,
    runtime: PathBuf,
}

fn cleanup_stale_pack_test_attempts(root: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let max_age = std::time::Duration::from_secs(
        attune_common::repositories::pack_install::PACK_INSTALL_ACTIVE_TTL_SECS as u64,
    );
    for entry in entries.flatten() {
        let path = entry.path();
        let is_attempt = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.parse::<i64>().is_ok());
        let is_stale_directory = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.is_dir())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if is_attempt && is_stale_directory {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                tracing::warn!(path = %path.display(), %error, "Failed to remove stale pack test attempt data");
            }
        }
    }
}

fn pack_candidate_token_matches(expected_hash: &str, candidate_token: &str) -> bool {
    attune_common::auth::hash_integration_token(candidate_token) == expected_hash
}

impl Drop for PackTestCandidateCleanup {
    fn drop(&mut self) {
        for path in self.files.iter().chain(std::iter::once(&self.runtime)) {
            if let Err(error) = std::fs::remove_dir_all(path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), %error, "Failed to remove pack test attempt data");
                }
            }
        }
    }
}

/// Run a pack's test suite on this worker and persist the outcome.
async fn handle_pack_test(
    db_pool: &PgPool,
    worker_id: i64,
    packs_base_dir: &std::path::Path,
    runtime_envs_dir: &std::path::Path,
    pack_transport: &dyn attune_common::pack_transport::PackFileTransport,
    payload: &PackTestRequestedPayload,
) -> Result<()> {
    use attune_common::test_executor::{TestConfig, TestExecutor};
    use std::path::PathBuf;

    validate_mq_pack_ref(&payload.pack_ref, "PackTestRequested")?;

    let Some(install) = PackInstallRepository::new(db_pool.clone())
        .find_by_id(payload.pack_install_id)
        .await?
    else {
        warn!(
            pack_install_id = payload.pack_install_id,
            "Ignoring pack test for a missing install"
        );
        return Ok(());
    };
    if install.status != PackInstallStatus::Running.as_str()
        || install.assigned_worker_id != Some(worker_id)
    {
        if install.status != PackInstallStatus::Running.as_str()
            && pack_transport.transport_mode() != "volume"
            && payload.candidate_path.is_some()
        {
            let _ = std::fs::remove_dir_all(
                packs_base_dir
                    .join(".pack-test-attempts")
                    .join(payload.pack_install_id.to_string()),
            );
            let _ = std::fs::remove_dir_all(
                runtime_envs_dir
                    .join(".pack-test-attempts")
                    .join(payload.pack_install_id.to_string()),
            );
        }
        info!(
            pack_install_id = payload.pack_install_id,
            status = %install.status,
            assigned_worker_id = ?install.assigned_worker_id,
            worker_id,
            "Ignoring stale or misrouted pack test delivery"
        );
        return Ok(());
    }
    if install.pack_ref != payload.pack_ref
        || install.pack_version != payload.pack_version
        || install.trigger_reason != payload.trigger_reason
    {
        mark_pack_install_failed(
            db_pool,
            payload,
            "Pack test delivery did not match its install record",
        )
        .await?;
        return Ok(());
    }
    let expects_candidate = install.candidate_access_token_hash.is_some();
    if expects_candidate != payload.candidate_path.is_some()
        || expects_candidate != payload.candidate_access_token.is_some()
    {
        mark_pack_install_failed(
            db_pool,
            payload,
            "Pack test delivery did not match the claimed candidate mode",
        )
        .await?;
        return Ok(());
    }

    if let (Some(expected_hash), Some(candidate_token)) = (
        install.candidate_access_token_hash.as_deref(),
        payload.candidate_access_token.as_deref(),
    ) {
        if !pack_candidate_token_matches(expected_hash, candidate_token) {
            warn!(
                pack_install_id = payload.pack_install_id,
                "Ignoring pack test delivery with an invalid candidate token"
            );
            return Ok(());
        }
    }

    let volume_candidate_dir =
        if payload.candidate_path.is_some() && pack_transport.transport_mode() == "volume" {
            match resolve_pack_test_dir(packs_base_dir, payload) {
                Ok(path) => Some(path),
                Err(error) => {
                    mark_pack_install_failed(db_pool, payload, &error.to_string()).await?;
                    return Ok(());
                }
            }
        } else {
            None
        };

    let _candidate_cleanup = payload
        .candidate_path
        .as_ref()
        .map(|_| PackTestCandidateCleanup {
            files: Some(if pack_transport.transport_mode() == "volume" {
                volume_candidate_dir
                    .clone()
                    .expect("volume candidate was validated")
            } else {
                packs_base_dir
                    .join(".pack-test-attempts")
                    .join(payload.pack_install_id.to_string())
            }),
            runtime: runtime_envs_dir
                .join(".pack-test-attempts")
                .join(payload.pack_install_id.to_string()),
        });

    info!(
        "Running pack tests for '{}' v{} (install {})",
        payload.pack_ref, payload.pack_version, payload.pack_install_id
    );

    let pack_dir =
        if payload.candidate_path.is_some() && pack_transport.transport_mode() != "volume" {
            match pack_transport
                .sync_pack_test_candidate(
                    &payload.pack_ref,
                    payload.pack_install_id,
                    payload.candidate_access_token.as_deref(),
                )
                .await
            {
                Ok(path) => path,
                Err(e) => {
                    mark_pack_install_failed(
                        db_pool,
                        payload,
                        &format!("Failed to sync candidate pack files before testing: {e}"),
                    )
                    .await?;
                    return Ok(());
                }
            }
        } else if let Some(candidate_dir) = volume_candidate_dir {
            candidate_dir
        } else {
            // Make sure the active pack files are present locally (API transport).
            if pack_transport.transport_mode() != "volume" {
                match pack_transport.sync_pack(&payload.pack_ref).await {
                    Ok(()) => info!("Pack '{}' synced for testing", payload.pack_ref),
                    Err(e) => {
                        mark_pack_install_failed(
                            db_pool,
                            payload,
                            &format!("Failed to sync pack files before testing: {e}"),
                        )
                        .await?;
                        return Ok(());
                    }
                }
            }
            packs_base_dir.join(&payload.pack_ref)
        };
    let pack_yaml_path = pack_dir.join("pack.yaml");
    if !pack_yaml_path.exists() {
        mark_pack_install_failed(
            db_pool,
            payload,
            &format!(
                "pack.yaml not found for pack '{}' on worker",
                payload.pack_ref
            ),
        )
        .await?;
        return Ok(());
    }

    let pack_yaml_content = tokio::fs::read_to_string(&pack_yaml_path)
        .await
        .map_err(|e| {
            Error::Internal(format!(
                "Failed to read pack.yaml for '{}': {e}",
                payload.pack_ref
            ))
        })?;
    let pack_yaml: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&pack_yaml_content).map_err(|e| {
            Error::Internal(format!(
                "Failed to parse pack.yaml for '{}': {e}",
                payload.pack_ref
            ))
        })?;

    let Some(testing_config) = pack_yaml.get("testing") else {
        // No testing configuration on the worker side; treat as a successful
        // (no-op) run so installation is not blocked by missing config.
        mark_pack_install_succeeded(db_pool, payload, None, None).await?;
        return Ok(());
    };

    let test_config: TestConfig =
        serde_yaml_ng::from_value(testing_config.clone()).map_err(|e| {
            Error::Internal(format!(
                "Failed to parse test configuration for '{}': {e}",
                payload.pack_ref
            ))
        })?;
    if let Err(error) = test_config.validate_policy() {
        mark_pack_install_failed(db_pool, payload, &error.to_string()).await?;
        return Ok(());
    }

    if !test_config.enabled {
        mark_pack_install_succeeded(db_pool, payload, None, None).await?;
        return Ok(());
    }

    let python_interpreter = if test_config
        .runners
        .values()
        .any(|runner| matches!(runner.r#type.as_str(), "unittest" | "pytest"))
    {
        Some(if payload.candidate_path.is_some() {
            env_setup::prepare_python_candidate_test_runtime(
                db_pool,
                &pack_dir,
                runtime_envs_dir,
                payload.pack_install_id,
            )
            .await?
        } else {
            env_setup::prepare_python_test_runtime(
                db_pool,
                worker_id,
                &payload.pack_ref,
                packs_base_dir,
                runtime_envs_dir,
            )
            .await?
        })
    } else {
        None
    };

    let executor = TestExecutor::new(PathBuf::from(packs_base_dir));
    let result = match executor
        .execute_pack_tests_at_with_python_interpreter(
            &pack_dir,
            &payload.pack_ref,
            &payload.pack_version,
            &test_config,
            python_interpreter.as_deref(),
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            mark_pack_install_failed(db_pool, payload, &format!("Test execution failed: {e}"))
                .await?;
            return Ok(());
        }
    };

    let result_json = serde_json::to_value(&result)?;
    let passed = test_config.accepts_result(&result);

    // Persist the pack_test_execution row when the pack still exists.
    let test_execution_id = match (
        install.pack_id,
        PackRepository::find_by_ref(db_pool, &payload.pack_ref).await?,
    ) {
        (Some(expected_pack_id), Some(pack)) if pack.id == expected_pack_id => Some(
            PackTestRepository::new(db_pool.clone())
                .create(
                    pack.id,
                    &payload.pack_version,
                    &payload.trigger_reason,
                    &result,
                )
                .await?
                .id,
        ),
        _ => None,
    };

    if passed {
        mark_pack_install_succeeded(db_pool, payload, test_execution_id, Some(result_json)).await?;
    } else {
        let message = format!(
            "Pack tests did not pass ({}/{} passed)",
            result.passed, result.total_tests
        );
        let _ = PackInstallRepository::new(db_pool.clone())
            .finish_running(
                payload.pack_install_id,
                PackInstallStatus::Failed,
                test_execution_id,
                Some(result_json),
                Some(message),
            )
            .await?;
    }

    info!(
        "Pack tests for '{}' completed: status={} ({}/{} passed)",
        payload.pack_ref, result.status, result.passed, result.total_tests
    );

    Ok(())
}

fn resolve_pack_test_dir(
    packs_base_dir: &std::path::Path,
    payload: &PackTestRequestedPayload,
) -> Result<PathBuf> {
    let active_dir = packs_base_dir.join(&payload.pack_ref);
    let Some(candidate_path) = &payload.candidate_path else {
        return Ok(active_dir);
    };

    let candidate_dir = PathBuf::from(candidate_path);
    let canonical_root = std::fs::canonicalize(packs_base_dir).map_err(|error| {
        Error::Internal(format!("Failed to resolve configured pack root: {error}"))
    })?;
    let canonical_candidate = std::fs::canonicalize(&candidate_dir).map_err(|error| {
        Error::Internal(format!("Failed to resolve pack test candidate: {error}"))
    })?;
    let expected_candidate = canonical_root.join(format!(".pack-test-{}", payload.pack_install_id));
    if canonical_candidate != expected_candidate {
        return Err(Error::Internal(format!(
            "Pack test candidate path is outside the configured pack root: {candidate_path}"
        )));
    }
    Ok(candidate_dir)
}

/// Mark a pack install record as succeeded.
async fn mark_pack_install_succeeded(
    db_pool: &PgPool,
    payload: &PackTestRequestedPayload,
    test_execution_id: Option<i64>,
    test_result: Option<serde_json::Value>,
) -> Result<()> {
    let status = if payload.trigger_reason == "manual" {
        PackInstallStatus::Succeeded
    } else {
        PackInstallStatus::Activating
    };
    let _ = PackInstallRepository::new(db_pool.clone())
        .finish_running(
            payload.pack_install_id,
            status,
            test_execution_id,
            test_result,
            None,
        )
        .await?;
    Ok(())
}

/// Mark a pack install record as failed with an error message.
async fn mark_pack_install_failed(
    db_pool: &PgPool,
    payload: &PackTestRequestedPayload,
    message: &str,
) -> Result<()> {
    let _ = PackInstallRepository::new(db_pool.clone())
        .finish_running(
            payload.pack_install_id,
            PackInstallStatus::Failed,
            None,
            None,
            Some(message.to_string()),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_name_format() {
        let worker_id = 42;
        let queue_name = format!("worker.{}.executions", worker_id);
        assert_eq!(queue_name, "worker.42.executions");
    }

    #[test]
    fn test_status_string_conversion() {
        let status = ExecutionStatus::Running;
        let status_str = match status {
            ExecutionStatus::Running => "running",
            _ => "unknown",
        };
        assert_eq!(status_str, "running");
    }

    #[test]
    fn candidate_test_directory_must_stay_under_pack_root() {
        let temp = tempfile::tempdir().unwrap();
        let packs = temp.path().join("packs");
        let candidate = packs.join(".pack-test-1");
        let wrong_candidate = packs.join(".pack-test-2");
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::create_dir_all(&wrong_candidate).unwrap();
        let mut payload = PackTestRequestedPayload {
            pack_install_id: 1,
            pack_ref: "demo".to_string(),
            pack_version: "1.0.0".to_string(),
            candidate_path: Some(candidate.to_string_lossy().to_string()),
            candidate_access_token: Some("candidate-token".to_string()),
            trigger_reason: "install".to_string(),
            required_runtimes: Vec::new(),
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
        };

        assert_eq!(resolve_pack_test_dir(&packs, &payload).unwrap(), candidate);
        payload.candidate_path = Some(wrong_candidate.to_string_lossy().to_string());
        assert!(resolve_pack_test_dir(&packs, &payload).is_err());
        payload.candidate_path = Some("/tmp/candidate".to_string());
        assert!(resolve_pack_test_dir(&packs, &payload).is_err());
    }

    #[test]
    fn candidate_test_token_must_match_the_claimed_attempt() {
        let expected = attune_common::auth::hash_integration_token("attempt-secret");
        assert!(pack_candidate_token_matches(&expected, "attempt-secret"));
        assert!(!pack_candidate_token_matches(&expected, "different-secret"));
    }

    #[test]
    fn test_execution_completed_payload_structure() {
        let payload = ExecutionCompletedPayload {
            execution_id: 123,
            action_id: 456,
            action_ref: "test.action".to_string(),
            status: "Completed".to_string(),
            result: Some(serde_json::json!({"output": "test"})),
            completed_at: Utc::now(),
        };

        assert_eq!(payload.execution_id, 123);
        assert_eq!(payload.action_id, 456);
        assert_eq!(payload.action_ref, "test.action");
        assert_eq!(payload.status, "Completed");
        assert!(payload.result.is_some());
    }

    // Test removed - ExecutionStatusPayload struct doesn't exist
    // #[test]
    // fn test_execution_status_payload_structure() {
    //     ...
    // }

    #[test]
    fn test_execution_scheduled_payload_structure() {
        let payload = ExecutionScheduledPayload {
            execution_id: 111,
            action_ref: "core.test".to_string(),
            worker_id: 222,
        };

        assert_eq!(payload.execution_id, 111);
        assert_eq!(payload.action_ref, "core.test");
        assert_eq!(payload.worker_id, 222);
    }

    #[test]
    fn test_status_format_for_completion() {
        let status = ExecutionStatus::Completed;
        let status_str = format!("{:?}", status);
        assert_eq!(status_str, "Completed");

        let status = ExecutionStatus::Failed;
        let status_str = format!("{:?}", status);
        assert_eq!(status_str, "Failed");

        let status = ExecutionStatus::Timeout;
        let status_str = format!("{:?}", status);
        assert_eq!(status_str, "Timeout");

        let status = ExecutionStatus::Cancelled;
        let status_str = format!("{:?}", status);
        assert_eq!(status_str, "Cancelled");
    }

    #[test]
    fn test_runtime_name_filtering_respects_worker_capabilities() {
        let worker_filter = vec!["python".to_string()];
        let runtime_names = vec![
            "python".to_string(),
            "node".to_string(),
            "python3".to_string(),
        ];

        let filtered = filter_runtime_names_for_worker(&runtime_names, Some(&worker_filter));

        assert_eq!(filtered, vec!["python".to_string(), "python3".to_string()]);
    }

    #[test]
    fn test_mq_pack_ref_validation_rejects_path_input() {
        assert!(validate_mq_pack_ref("valid_pack", "PackRegistered").is_ok());
        assert!(validate_mq_pack_ref("org.pack", "PackRegistered").is_err());
        assert!(validate_mq_pack_ref("../escape", "PackRegistered").is_err());
        assert!(validate_mq_pack_ref("pack/name", "PackDeleted").is_err());
    }

    #[tokio::test]
    async fn test_drain_completed_in_flight_tasks_only_reaps_finished_entries() {
        let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let mut tasks = JoinSet::new();
        tasks.spawn(async {
            completed_tx.send(()).unwrap();
        });
        tasks.spawn(async {
            release_rx.await.unwrap();
        });

        completed_rx.await.unwrap();

        let drained = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let drained = WorkerService::drain_completed_in_flight_tasks(&mut tasks);
                if drained > 0 {
                    break drained;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completed task was not ready to reap");
        assert_eq!(drained, 1);
        assert_eq!(tasks.len(), 1);

        release_tx.send(()).unwrap();
        let remaining = tasks.join_next().await;
        assert!(remaining.unwrap().is_ok());
        assert_eq!(tasks.len(), 0);
    }
}
