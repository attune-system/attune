//! Sensor Manager
//!
//! Manages the lifecycle of standalone sensor processes including loading,
//! starting, stopping, and monitoring sensor instances.
//!
//! All sensors are independent processes that communicate with the API
//! to create events. The sensor manager is responsible for:
//! - Starting sensor processes when rules become active
//! - Stopping sensor processes when no rules need them
//! - Provisioning authentication tokens for sensor processes
//! - Monitoring sensor health and restarting failed sensors

use anyhow::{anyhow, Result};
use attune_common::artifact_transport::{sync_local_file_to_transport, ArtifactFileTransport};
use attune_common::auth::WorkerTokenProvider;
use attune_common::models::{
    enums::OwnerType, runtime::RuntimeExecutionConfig, Id, Sensor, SensorProcess,
    SensorProcessStatus, Trigger,
};
use attune_common::mq::{Connection, Publisher, PublisherConfig};
use attune_common::repositories::{
    sensor_process::{
        MarkSensorProcessFailedInput, MarkSensorProcessStoppedInput,
        RecordSensorProcessAlertedInput, UpsertSensorProcessStartInput,
    },
    ArtifactRepository, ArtifactVersionRepository, FindById, List, RuntimeRepository,
    RuntimeVersionRepository, SensorProcessRepository, SensorRepository, TriggerRepository,
    WorkerRepository,
};
use attune_common::runtime_detection::normalize_runtime_name;
use attune_common::scheduling::{
    worker_labels_from_capabilities, worker_matches_placement, worker_taints_from_capabilities,
};
use attune_common::system_alert::{emit_core_alert, SystemAlert};
use attune_common::version_matching::select_best_version;
use chrono::Utc;

use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::io;
#[cfg(test)]
use std::io::SeekFrom;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
#[cfg(test)]
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout, Duration};
use tracing::{debug, error, info, warn};

use crate::api_client::ApiClient;

const SENSOR_RESTART_BASE_DELAY: Duration = Duration::from_secs(5);
const SENSOR_RESTART_MAX_DELAY: Duration = Duration::from_secs(300);
const SENSOR_ALERT_FAILURE_THRESHOLD: i32 = 3;
const STDERR_EXCERPT_MAX_BYTES: u64 = 16 * 1024;
const STDERR_EXCERPT_MAX_LINES: usize = 80;

fn existing_command_env(cmd: &Command, key: &str) -> Option<String> {
    cmd.as_std()
        .get_envs()
        .find_map(|(env_key, value)| {
            if env_key == key {
                value.map(|value| value.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .or_else(|| std::env::var(key).ok())
}

fn apply_runtime_env_vars(
    cmd: &mut Command,
    exec_config: &RuntimeExecutionConfig,
    pack_dir: &std::path::Path,
    env_dir: Option<&std::path::Path>,
) {
    if exec_config.env_vars.is_empty() {
        return;
    }

    let vars = exec_config.build_template_vars_with_env(pack_dir, env_dir);
    for (key, env_var_config) in &exec_config.env_vars {
        let resolved = env_var_config.resolve(&vars, existing_command_env(cmd, key).as_deref());
        debug!("Setting sensor runtime env var: {}={}", key, resolved);
        cmd.env(key, resolved);
    }
}

fn collect_sensor_token_trigger_types(triggers: &[Trigger]) -> Vec<String> {
    let mut trigger_types: Vec<String> = triggers
        .iter()
        .map(|trigger| trigger.r#ref.trim())
        .filter(|trigger_ref| !trigger_ref.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    trigger_types.sort();
    trigger_types.dedup();
    trigger_types
}

fn configure_sensor_process(cmd: &mut Command) -> io::Result<()> {
    #[cfg(unix)]
    {
        // Run each sensor in its own process group so shutdown can terminate
        // the interpreter wrapper and any children it spawned.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }

    Ok(())
}

fn signal_process_group_or_process(pid: u32, signal: i32) {
    #[cfg(unix)]
    {
        let pgid = -(pid as i32);
        let rc = unsafe { libc::kill(pgid, signal) };
        if rc == 0 {
            return;
        }

        let err = io::Error::last_os_error();
        warn!(
            "Failed to signal sensor process group {} with signal {}: {}. Falling back to PID {}",
            pid, signal, err, pid
        );
    }

    unsafe {
        libc::kill(pid as i32, signal);
    }
}

fn exit_signal(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }

    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

fn active_rule_count_i32(count: i64) -> i32 {
    count.clamp(0, i32::MAX as i64) as i32
}

fn sensor_restart_backoff_delay(failure_count: i32) -> Duration {
    let exponent = failure_count.max(1).saturating_sub(1).min(10) as u32;
    let multiplier = 2_u64.saturating_pow(exponent);
    let secs = SENSOR_RESTART_BASE_DELAY
        .as_secs()
        .saturating_mul(multiplier)
        .min(SENSOR_RESTART_MAX_DELAY.as_secs());
    Duration::from_secs(secs)
}

fn duration_to_chrono(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration)
        .unwrap_or_else(|_| chrono::Duration::seconds(SENSOR_RESTART_MAX_DELAY.as_secs() as i64))
}

#[cfg(test)]
async fn read_sensor_stderr_excerpt_from_artifacts_dir(
    artifacts_dir: &std::path::Path,
    sensor_ref: &str,
) -> Option<String> {
    let path = artifacts_dir
        .join("sensors")
        .join(sensor_ref)
        .join("stderr.log");

    let mut file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(e) => {
            debug!(
                "No stderr excerpt available for sensor {} from {}: {}",
                sensor_ref,
                path.display(),
                e
            );
            return None;
        }
    };

    let len = match file.metadata().await {
        Ok(meta) => meta.len(),
        Err(e) => {
            warn!(
                "Failed to stat stderr log for sensor {} at {}: {}",
                sensor_ref,
                path.display(),
                e
            );
            return None;
        }
    };

    let start = len.saturating_sub(STDERR_EXCERPT_MAX_BYTES);
    if let Err(e) = file.seek(SeekFrom::Start(start)).await {
        warn!(
            "Failed to seek stderr log for sensor {} at {}: {}",
            sensor_ref,
            path.display(),
            e
        );
        return None;
    }

    let mut bytes = Vec::with_capacity((len - start) as usize);
    if let Err(e) = file.read_to_end(&mut bytes).await {
        warn!(
            "Failed to read stderr log for sensor {} at {}: {}",
            sensor_ref,
            path.display(),
            e
        );
        return None;
    }

    format_stderr_excerpt(&bytes, start > 0)
}

async fn read_sensor_stderr_excerpt_from_transport(
    transport: &dyn ArtifactFileTransport,
    sensor_ref: &str,
) -> Option<String> {
    let relative_path = format!("sensors/{}/stderr.log", sensor_ref);
    let bytes = match transport.read_file(&relative_path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            debug!(
                "No stderr excerpt available for sensor {} from {} transport path {}: {}",
                sensor_ref,
                transport.transport_mode(),
                relative_path,
                e
            );
            return None;
        }
    };

    let start = bytes
        .len()
        .saturating_sub(STDERR_EXCERPT_MAX_BYTES as usize);
    format_stderr_excerpt(&bytes[start..], start > 0)
}

fn format_stderr_excerpt(bytes: &[u8], truncated: bool) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<&str> = text.lines().rev().take(STDERR_EXCERPT_MAX_LINES).collect();
    lines.reverse();
    let excerpt = lines.join("\n");
    if excerpt.trim().is_empty() {
        None
    } else if truncated {
        Some(format!("…\n{}", excerpt))
    } else {
        Some(excerpt)
    }
}

async fn terminate_sensor_child(child: &mut Child, sensor_label: &str) {
    let Some(pid) = child.id() else {
        if let Err(e) = child.start_kill() {
            error!("Failed to kill sensor process {}: {}", sensor_label, e);
            return;
        }
        if let Err(e) = child.wait().await {
            error!("Failed to reap sensor process {}: {}", sensor_label, e);
        }
        return;
    };

    info!(
        "Sending SIGTERM to sensor {} process group {}",
        sensor_label, pid
    );
    signal_process_group_or_process(pid, libc::SIGTERM);

    match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => error!("Failed waiting for sensor process {}: {}", sensor_label, e),
        Err(_) => {
            warn!(
                "Sensor {} did not exit after SIGTERM + 5s, sending SIGKILL",
                sensor_label
            );
            signal_process_group_or_process(pid, libc::SIGKILL);
            if let Err(e) = child.wait().await {
                error!(
                    "Failed to reap sensor process {} after SIGKILL: {}",
                    sensor_label, e
                );
            }
        }
    }
}

/// Sensor manager that coordinates all sensor instances
#[derive(Clone)]
pub struct SensorManager {
    inner: Arc<SensorManagerInner>,
}

pub struct SensorManagerConfig {
    pub api_url: String,
    pub worker_token_provider: Option<Arc<WorkerTokenProvider>>,
    pub notifier_ws_url: String,
    pub mq_url: String,
    pub packs_base_dir: String,
    pub runtime_envs_dir: String,
    pub artifact_transport: Arc<dyn ArtifactFileTransport>,
    pub sensor_log_config: crate::sensor_log::SensorLogConfig,
}

#[derive(Debug, Clone, Default)]
pub struct SensorActivityMetrics {
    pub monitored_sensors: u64,
    pub running_sensors: u64,
    pub active_rules: u64,
}

#[derive(Debug)]
struct ExitedSensorProcess {
    sensor: Sensor,
    status: Arc<RwLock<SensorStatus>>,
    exit_code: Option<i32>,
    signal: Option<i32>,
}

struct SensorManagerInner {
    db: PgPool,
    sensors: Arc<RwLock<HashMap<Id, SensorInstance>>>,
    running: Arc<RwLock<bool>>,
    packs_base_dir: String,
    runtime_envs_dir: String,
    artifact_transport: Arc<dyn ArtifactFileTransport>,
    sensor_log_config: crate::sensor_log::SensorLogConfig,
    api_client: ApiClient,
    api_url: String,
    notifier_ws_url: String,
    mq_url: String,
    /// Worker ID for this sensor service instance (set after registration).
    /// Used to read locally-detected runtime versions from the worker row
    /// when resolving `sensor.runtime_version_constraint`. Zero means unset.
    worker_id: AtomicI64,
}

impl SensorManager {
    /// Create a new sensor manager
    pub fn new(db: PgPool, config: SensorManagerConfig) -> Self {
        // Create API client for token provisioning via authenticated internal endpoint.
        let api_client = ApiClient::new(
            config.api_url.clone(),
            None,
            config.worker_token_provider,
        );

        Self {
            inner: Arc::new(SensorManagerInner {
                db,
                sensors: Arc::new(RwLock::new(HashMap::new())),
                running: Arc::new(RwLock::new(false)),
                packs_base_dir: config.packs_base_dir,
                runtime_envs_dir: config.runtime_envs_dir,
                artifact_transport: config.artifact_transport,
                sensor_log_config: config.sensor_log_config,
                api_client,
                api_url: config.api_url,
                notifier_ws_url: config.notifier_ws_url,
                mq_url: config.mq_url,
                worker_id: AtomicI64::new(0),
            }),
        }
    }

    /// Record the worker ID assigned to this sensor service after
    /// registration. Required for `runtime_version_constraint` resolution
    /// (we read this worker's `capabilities.runtime_versions` to filter
    /// candidate runtime versions to those locally available).
    pub fn set_worker_id(&self, worker_id: Id) {
        self.inner.worker_id.store(worker_id, Ordering::SeqCst);
        info!("Sensor manager bound to worker {}", worker_id);
    }

    /// Start the sensor manager
    pub async fn start(&self) -> Result<()> {
        info!("Starting sensor manager");

        // Mark as running
        *self.inner.running.write().await = true;

        // Load and start all enabled sensors with active rules
        let sensors = self.load_enabled_sensors().await?;
        info!("Loaded {} enabled sensor(s)", sensors.len());

        for sensor in sensors {
            // Only start sensors that have active rules (across all their triggers)
            match self.sensor_has_active_rules(sensor.id).await {
                Ok(true) => {
                    let count = self.sensor_active_rule_count(sensor.id).await.unwrap_or(0);
                    info!(
                        "Starting sensor {} - has {} active rule(s)",
                        sensor.r#ref, count
                    );
                    if let Err(e) = self.start_sensor(sensor, true).await {
                        error!("Failed to start sensor: {}", e);
                    }
                }
                Ok(false) => {
                    info!("Skipping sensor {} - no active rules", sensor.r#ref);
                }
                Err(e) => {
                    error!(
                        "Failed to check active rules for sensor {}: {}",
                        sensor.r#ref, e
                    );
                }
            }
        }

        // Start monitoring loop
        let manager = self.clone();
        tokio::spawn(async move {
            manager.monitoring_loop().await;
        });

        // Rule lifecycle messages are the fast path; this reconciliation loop
        // covers missed messages or deployments where the API publisher is down.
        let manager = self.clone();
        tokio::spawn(async move {
            manager.lifecycle_reconciliation_loop().await;
        });

        info!("Sensor manager started");

        Ok(())
    }

    /// Stop the sensor manager
    pub async fn stop(&self) -> Result<()> {
        info!("Stopping sensor manager");

        // Mark as not running
        *self.inner.running.write().await = false;

        // Collect sensor IDs to stop
        let sensor_ids: Vec<Id> = self.inner.sensors.read().await.keys().copied().collect();

        // Stop all sensors
        for sensor_id in sensor_ids {
            info!("Stopping sensor {}", sensor_id);
            if let Err(e) = self.stop_sensor(sensor_id).await {
                error!("Failed to stop sensor {}: {}", sensor_id, e);
            }
        }

        info!("Sensor manager stopped");

        Ok(())
    }

    /// Load all enabled sensors from the database
    async fn load_enabled_sensors(&self) -> Result<Vec<Sensor>> {
        use attune_common::repositories::SensorRepository;

        let all_sensors = SensorRepository::list(&self.inner.db).await?;
        let enabled_sensors: Vec<Sensor> = all_sensors.into_iter().filter(|s| s.enabled).collect();
        Ok(enabled_sensors)
    }

    async fn ensure_runtime_environment(
        &self,
        exec_config: &RuntimeExecutionConfig,
        pack_dir: &std::path::Path,
        env_dir: &std::path::Path,
    ) -> Result<()> {
        let env_cfg = match &exec_config.environment {
            Some(cfg) if cfg.env_type != "none" => cfg,
            _ => return Ok(()),
        };

        let vars = exec_config.build_template_vars_with_env(pack_dir, Some(env_dir));

        if !env_dir.exists() {
            if env_cfg.create_command.is_empty() {
                return Err(anyhow!(
                    "Runtime environment '{}' requires create_command but none is configured",
                    env_cfg.env_type
                ));
            }

            if let Some(parent) = env_dir.parent() {
                attune_common::utils::create_shared_dir_all(parent)
                    .await
                    .map_err(|e| {
                        anyhow!(
                            "Failed to create runtime environment parent directory {}: {}",
                            parent.display(),
                            e
                        )
                    })?;
            }

            let resolved_cmd =
                RuntimeExecutionConfig::resolve_command(&env_cfg.create_command, &vars);
            let (program, args) = resolved_cmd
                .split_first()
                .ok_or_else(|| anyhow!("Empty create_command for runtime environment"))?;

            info!(
                "Creating sensor runtime environment at {}: {:?}",
                env_dir.display(),
                resolved_cmd
            );

            let output = Command::new(program)
                .args(args)
                .current_dir(pack_dir)
                .output()
                .await
                .map_err(|e| anyhow!("Failed to run create command '{}': {}", program, e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!(
                    "Runtime environment creation failed (exit {}): {}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim()
                ));
            }
        }

        let dep_cfg = match &exec_config.dependencies {
            Some(cfg) => cfg,
            None => return Ok(()),
        };

        let manifest_path = pack_dir.join(&dep_cfg.manifest_file);
        if !manifest_path.exists() || dep_cfg.install_command.is_empty() {
            return Ok(());
        }

        let install_marker = env_dir.join(".attune_sensor_deps_installed");
        if install_marker.exists() {
            return Ok(());
        }

        let resolved_cmd = RuntimeExecutionConfig::resolve_command(&dep_cfg.install_command, &vars);
        let (program, args) = resolved_cmd
            .split_first()
            .ok_or_else(|| anyhow!("Empty install_command for runtime dependencies"))?;

        info!(
            "Installing sensor runtime dependencies for {} using {:?}",
            pack_dir.display(),
            resolved_cmd
        );

        let output = Command::new(program)
            .args(args)
            .current_dir(pack_dir)
            .output()
            .await
            .map_err(|e| {
                anyhow!(
                    "Failed to run dependency install command '{}': {}",
                    program,
                    e
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "Runtime dependency installation failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            ));
        }

        tokio::fs::write(&install_marker, b"ok")
            .await
            .map_err(|e| {
                anyhow!(
                    "Failed to write dependency install marker {}: {}",
                    install_marker.display(),
                    e
                )
            })?;

        Ok(())
    }

    /// Start a sensor instance
    async fn start_sensor(&self, sensor: Sensor, reset_failure_count: bool) -> Result<()> {
        info!("Starting sensor {} ({})", sensor.r#ref, sensor.id);

        if self.sensor_instance_running(sensor.id).await {
            info!("Sensor {} is already running, skipping start", sensor.r#ref);
            return Ok(());
        }

        // Load all triggers this sensor can emit so token scope remains aligned
        // even when some triggers are currently disabled.
        let sensor_triggers: Vec<_> = TriggerRepository::find_by_sensor(&self.inner.db, sensor.id)
            .await
            .map_err(|e| anyhow!("Failed to load triggers for sensor {}: {}", sensor.r#ref, e))?;

        let token_trigger_types = collect_sensor_token_trigger_types(&sensor_triggers);
        if token_trigger_types.is_empty() {
            warn!(
                "Sensor {} has no valid trigger refs for token scope, skipping start",
                sensor.r#ref
            );
            return Ok(());
        }

        let enabled_triggers: Vec<_> = sensor_triggers
            .into_iter()
            .filter(|trigger| trigger.enabled)
            .collect();

        if enabled_triggers.is_empty() {
            warn!(
                "Sensor {} has no enabled associated triggers, skipping start",
                sensor.r#ref
            );
            return Ok(());
        }

        // All sensors are now standalone processes
        let instance = self
            .start_standalone_sensor(
                sensor.clone(),
                enabled_triggers,
                token_trigger_types,
                reset_failure_count,
            )
            .await?;

        // Store instance
        self.inner.sensors.write().await.insert(sensor.id, instance);

        info!("Sensor {} started successfully", sensor.r#ref);

        Ok(())
    }

    /// Start a standalone sensor with token provisioning
    async fn start_standalone_sensor(
        &self,
        sensor: Sensor,
        triggers: Vec<Trigger>,
        token_trigger_types: Vec<String>,
        reset_failure_count: bool,
    ) -> Result<SensorInstance> {
        info!("Starting standalone sensor: {}", sensor.r#ref);

        // Provision sensor token via API
        info!(
            "Provisioning token for sensor {} with {} trigger scope ref(s)",
            sensor.r#ref,
            token_trigger_types.len()
        );
        let token_response = self
            .inner
            .api_client
            .create_sensor_token(&sensor.r#ref, token_trigger_types, Some(86400))
            .await
            .map_err(|e| anyhow!("Failed to provision sensor token: {}", e))?;

        info!(
            "Token provisioned for sensor {} (expires: {})",
            sensor.r#ref, token_response.expires_at
        );

        // Build sensor script path
        let pack_ref = sensor
            .pack_ref
            .as_ref()
            .ok_or_else(|| anyhow!("Sensor {} has no pack_ref", sensor.r#ref))?;

        // Skip sensors with protocol-based entrypoints (e.g., internal://timer)
        // These are placeholders created by tests and have no corresponding binary.
        if sensor.entrypoint.contains("://") {
            return Err(anyhow!(
                "Sensor '{}' has protocol-based entrypoint '{}' which cannot be spawned as a process. \
                 Use the core pack's built-in sensor instead.",
                sensor.r#ref, sensor.entrypoint
            ));
        }

        let sensor_script = format!(
            "{}/{}/sensors/{}",
            self.inner.packs_base_dir, pack_ref, sensor.entrypoint
        );

        // Load the runtime to determine how to execute the sensor
        let runtime = RuntimeRepository::find_by_id(&self.inner.db, sensor.runtime)
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "Runtime {} not found for sensor {}",
                    sensor.runtime,
                    sensor.r#ref
                )
            })?;

        // Resolve runtime version constraint. If the sensor declares one,
        // pick the best locally-available version from the runtime_version
        // table, filtered against this sensor worker's reported runtime_versions.
        // If no compatible version is available locally, refuse to start the
        // sensor so a sensor worker that does have it can pick it up.
        let (version_config_override, version_env_suffix, selected_version) =
            self.resolve_runtime_version(&runtime, &sensor).await?;

        let exec_config = version_config_override
            .clone()
            .unwrap_or_else(|| runtime.parsed_execution_config());
        let rt_name = runtime.name.to_lowercase();
        let base_runtime_suffix = runtime
            .r#ref
            .rsplit('.')
            .next()
            .filter(|suffix| !suffix.is_empty())
            .unwrap_or(&rt_name)
            .to_string();
        let runtime_env_suffix = version_env_suffix.unwrap_or(base_runtime_suffix);

        info!(
            "Sensor {} runtime details: id={}, ref='{}', name='{}', selected_version={:?}, env_suffix='{}'",
            sensor.r#ref,
            runtime.id,
            runtime.r#ref,
            runtime.name,
            selected_version,
            runtime_env_suffix,
        );

        // Resolve the interpreter: check for a virtualenv/node_modules first,
        // then fall back to the system interpreter.
        let pack_dir = std::path::PathBuf::from(&self.inner.packs_base_dir).join(pack_ref);
        let env_dir = std::path::PathBuf::from(&self.inner.runtime_envs_dir)
            .join(pack_ref)
            .join(&runtime_env_suffix);
        if let Err(e) = self
            .ensure_runtime_environment(&exec_config, &pack_dir, &env_dir)
            .await
        {
            warn!(
                "Failed to ensure sensor runtime environment for {} at {}: {}",
                sensor.r#ref,
                env_dir.display(),
                e
            );
        }

        let env_dir_opt = if env_dir.exists() {
            Some(env_dir.as_path())
        } else {
            None
        };

        // Determine whether we need an interpreter or can execute directly.
        // Determine native vs interpreted purely from the runtime's execution_config.
        // A native runtime (e.g., core.native) has no interpreter configured —
        // its binary field is empty. Interpreted runtimes (Python, Node, etc.)
        // declare their interpreter binary explicitly in execution_config.
        let interpreter_binary = &exec_config.interpreter.binary;
        let is_native = interpreter_binary.is_empty()
            || interpreter_binary == "native"
            || interpreter_binary == "none";

        info!(
            "Sensor {} runtime={} (ref={}) interpreter='{}' native={} env_dir_exists={}",
            sensor.r#ref,
            rt_name,
            runtime.r#ref,
            interpreter_binary,
            is_native,
            env_dir.exists()
        );
        info!("Starting standalone sensor process: {}", sensor_script);

        // Fetch trigger instances (enabled rules with their trigger params) for ALL triggers
        info!(
            "About to fetch trigger instances for sensor {} ({} trigger(s))",
            sensor.r#ref,
            triggers.len()
        );
        let mut trigger_instances = Vec::new();
        for trig in &triggers {
            match self.fetch_trigger_instances(trig.id).await {
                Ok(instances) => {
                    info!(
                        "Fetched {} trigger instance(s) for trigger {} (sensor {})",
                        instances.len(),
                        trig.r#ref,
                        sensor.r#ref
                    );
                    trigger_instances.extend(instances);
                }
                Err(e) => {
                    error!(
                        "Failed to fetch trigger instances for trigger {} (sensor {}): {}",
                        trig.r#ref, sensor.r#ref, e
                    );
                    return Err(e);
                }
            }
        }

        let trigger_instances_json = serde_json::to_string(&trigger_instances)
            .map_err(|e| anyhow!("Failed to serialize trigger instances: {}", e))?;
        info!("Trigger instances JSON: {}", trigger_instances_json);

        // Build the command: use the interpreter for non-native runtimes,
        // execute the script directly for native binaries.
        let (spawn_binary, mut cmd) = if is_native {
            (sensor_script.clone(), Command::new(&sensor_script))
        } else {
            let resolved_interpreter =
                exec_config.resolve_interpreter_with_env(&pack_dir, env_dir_opt);
            info!(
                "Using interpreter {} for sensor {}",
                resolved_interpreter.display(),
                sensor.r#ref
            );
            let binary_str = resolved_interpreter.display().to_string();
            let mut c = Command::new(&resolved_interpreter);
            // Pass any extra interpreter args (e.g., -u for unbuffered Python)
            for arg in &exec_config.interpreter.args {
                c.arg(arg);
            }
            c.arg(&sensor_script);
            (binary_str, c)
        };

        // Log the full command for diagnostics
        info!(
            "Spawning sensor {}: binary='{}' is_native={} script='{}'",
            sensor.r#ref, spawn_binary, is_native, sensor_script
        );

        // Pre-flight check: verify the binary exists and is accessible
        let spawn_path = std::path::Path::new(&spawn_binary);
        if spawn_path.is_absolute() || spawn_path.components().count() > 1 {
            // Absolute or relative path with directory component — check it directly
            match std::fs::metadata(spawn_path) {
                Ok(meta) => {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = meta.permissions().mode();
                    let is_exec = mode & 0o111 != 0;
                    if !is_exec {
                        error!(
                            "Binary '{}' exists but is not executable (mode: {:o}). \
                             Sensor runtime ref='{}', execution_config interpreter='{}'.",
                            spawn_binary, mode, runtime.r#ref, interpreter_binary
                        );
                    }
                }
                Err(e) => {
                    error!(
                        "Cannot access binary '{}': {}. \
                         Sensor runtime ref='{}', execution_config interpreter='{}'.",
                        spawn_binary, e, runtime.r#ref, interpreter_binary
                    );
                }
            }
        }

        // Start the standalone sensor with token and configuration
        // Pass sensor ref (e.g., "core.interval_timer_sensor") for proper identification
        cmd.env("ATTUNE_API_URL", &self.inner.api_url)
            .env("ATTUNE_API_TOKEN", &token_response.token)
            .env("ATTUNE_SENSOR_ID", sensor.id.to_string())
            .env("ATTUNE_SENSOR_REF", &sensor.r#ref)
            .env("ATTUNE_SENSOR_TRIGGERS", &trigger_instances_json)
            .env("ATTUNE_NOTIFIER_WS_URL", &self.inner.notifier_ws_url)
            .env("ATTUNE_MQ_URL", &self.inner.mq_url)
            .env("ATTUNE_MQ_EXCHANGE", "attune.events")
            .env(
                "ATTUNE_ARTIFACTS_DIR",
                self.inner.artifact_transport.base_dir(),
            )
            .env("ATTUNE_LOG_LEVEL", "info");

        apply_runtime_env_vars(&mut cmd, &exec_config, &pack_dir, env_dir_opt);
        configure_sensor_process(&mut cmd)
            .map_err(|e| anyhow!("Failed to configure sensor process: {}", e))?;

        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                anyhow!(
                    "Failed to start sensor process for '{}': {} \
                     (binary='{}', is_native={}, runtime_ref='{}', \
                     interpreter_config='{}', env_dir='{}')",
                    sensor.r#ref,
                    e,
                    spawn_binary,
                    is_native,
                    runtime.r#ref,
                    interpreter_binary,
                    env_dir.display()
                )
            })?;

        // Get stdout and stderr for logging (standalone sensors output JSON logs to stdout)
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture sensor stdout"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to capture sensor stderr"))?;

        // Spawn tasks that write to rotating log files AND forward to tracing
        let log_config = self
            .inner
            .sensor_log_config
            .with_retention_overrides(sensor.log_retention_policy, sensor.log_retention_limit);
        // Register sensor log artifacts in DB before the log tasks start so
        // written segments can be associated with artifact versions.
        let log_artifacts = match crate::sensor_log::register_sensor_log_artifacts(
            &self.inner.db,
            &sensor.r#ref,
            self.inner.artifact_transport.as_ref(),
            &log_config,
        )
        .await
        {
            Ok(artifacts) => Some(artifacts),
            Err(e) => {
                warn!(
                    "Failed to register sensor log artifacts for '{}': {}",
                    sensor.r#ref, e
                );
                None
            }
        };

        let stdout_handle = crate::sensor_log::spawn_stdout_log_task(
            stdout,
            sensor.r#ref.clone(),
            self.inner.artifact_transport.clone(),
            log_config.clone(),
            log_artifacts.as_ref().map(|_| self.inner.db.clone()),
            log_artifacts
                .as_ref()
                .map(|artifacts| artifacts.stdout.clone()),
        );
        let stderr_handle = crate::sensor_log::spawn_stderr_log_task(
            stderr,
            sensor.r#ref.clone(),
            self.inner.artifact_transport.clone(),
            log_config.clone(),
            log_artifacts.as_ref().map(|_| self.inner.db.clone()),
            log_artifacts
                .as_ref()
                .map(|artifacts| artifacts.stderr.clone()),
        );

        self.persist_sensor_process_started(&sensor, child.id(), reset_failure_count)
            .await;

        Ok(SensorInstance::new_standalone(
            sensor,
            child,
            stdout_handle,
            stderr_handle,
        ))
    }

    async fn persist_sensor_process_started(
        &self,
        sensor: &Sensor,
        pid: Option<u32>,
        reset_failure_count: bool,
    ) {
        let worker_id = self.inner.worker_id.load(Ordering::SeqCst);
        if worker_id <= 0 {
            warn!(
                "Skipping sensor_process start persistence for {}: sensor worker_id is not set",
                sensor.r#ref
            );
            return;
        }

        let worker = match WorkerRepository::find_by_id(&self.inner.db, worker_id).await {
            Ok(Some(worker)) => worker,
            Ok(None) => {
                warn!(
                    "Skipping sensor_process start persistence for {}: worker {} not found",
                    sensor.r#ref, worker_id
                );
                return;
            }
            Err(e) => {
                warn!(
                    "Skipping sensor_process start persistence for {}: failed to load worker {}: {}",
                    sensor.r#ref, worker_id, e
                );
                return;
            }
        };

        let active_rule_count = match self.sensor_active_rule_count(sensor.id).await {
            Ok(count) => active_rule_count_i32(count),
            Err(e) => {
                warn!(
                    "Failed to count active rules while persisting sensor_process start for {}: {}",
                    sensor.r#ref, e
                );
                0
            }
        };

        if let Err(e) = SensorProcessRepository::upsert_starting_or_running(
            &self.inner.db,
            UpsertSensorProcessStartInput {
                sensor: sensor.id,
                sensor_ref: sensor.r#ref.clone(),
                worker: worker_id,
                worker_name: worker.name,
                status: SensorProcessStatus::Running,
                pid: pid.and_then(|p| i32::try_from(p).ok()),
                started_at: Some(Utc::now()),
                active_rule_count,
                log_artifact_ref: Some(format!("sensor.{}.stderr", sensor.r#ref)),
                meta: Some(serde_json::json!({
                    "manager": "attune-sensor",
                    "reset_failure_count": reset_failure_count,
                })),
                reset_failure_count,
            },
        )
        .await
        {
            warn!(
                "Failed to persist sensor_process start for {} on worker {}: {}",
                sensor.r#ref, worker_id, e
            );
        }
    }

    async fn persist_sensor_process_stopped(&self, sensor: &Sensor) {
        let worker_id = self.inner.worker_id.load(Ordering::SeqCst);
        if worker_id <= 0 {
            warn!(
                "Skipping sensor_process stop persistence for {}: sensor worker_id is not set",
                sensor.r#ref
            );
            return;
        }

        let active_rule_count = match self.sensor_active_rule_count(sensor.id).await {
            Ok(count) => Some(active_rule_count_i32(count)),
            Err(e) => {
                warn!(
                    "Failed to count active rules while persisting sensor_process stop for {}: {}",
                    sensor.r#ref, e
                );
                None
            }
        };

        if let Err(e) = SensorProcessRepository::mark_stopped(
            &self.inner.db,
            MarkSensorProcessStoppedInput {
                sensor: sensor.id,
                worker: worker_id,
                stopped_at: Some(Utc::now()),
                active_rule_count,
            },
        )
        .await
        {
            warn!(
                "Failed to persist sensor_process stop for {} on worker {}: {}",
                sensor.r#ref, worker_id, e
            );
        }
    }

    async fn read_sensor_stderr_excerpt(&self, sensor_ref: &str) -> Option<String> {
        read_sensor_stderr_excerpt_from_transport(
            self.inner.artifact_transport.as_ref(),
            sensor_ref,
        )
        .await
    }

    async fn sensor_instance_running(&self, sensor_id: Id) -> bool {
        let instance_state = {
            let sensors = self.inner.sensors.read().await;
            sensors
                .get(&sensor_id)
                .map(|instance| (instance.status.clone(), instance.child_process.is_some()))
        };

        let Some((status, has_child)) = instance_state else {
            return false;
        };

        let status = status.read().await;
        has_child && status.running && !status.failed
    }

    async fn sync_sensor_file_artifacts(&self, sensor: &Sensor) {
        if self.inner.artifact_transport.transport_mode() == "volume" {
            return;
        }

        let versions = match ArtifactVersionRepository::find_file_versions_by_scope_and_owner(
            &self.inner.db,
            OwnerType::Sensor,
            &sensor.r#ref,
        )
        .await
        {
            Ok(versions) => versions,
            Err(e) => {
                warn!(
                    "Failed to list file-backed artifacts for sensor '{}': {}",
                    sensor.r#ref, e
                );
                return;
            }
        };

        if versions.is_empty() {
            return;
        }

        let artifacts_dir = std::path::Path::new(self.inner.artifact_transport.base_dir());
        let mut synced_count = 0usize;

        for version in versions {
            let Some(file_path) = version.file_path.as_deref() else {
                continue;
            };

            let size = match sync_local_file_to_transport(
                artifacts_dir,
                self.inner.artifact_transport.as_ref(),
                file_path,
                version.content_type.as_deref(),
            )
            .await
            {
                Ok(Some(size)) => size as i64,
                Ok(None) => continue,
                Err(e) => {
                    warn!(
                        "Failed to copy local sensor artifact '{}' for sensor '{}' to {} transport: {}",
                        file_path,
                        sensor.r#ref,
                        self.inner.artifact_transport.transport_mode(),
                        e,
                    );
                    continue;
                }
            };

            if let Err(e) =
                ArtifactVersionRepository::update_size_bytes(&self.inner.db, version.id, size).await
            {
                warn!(
                    "Failed to update size_bytes for sensor artifact version {}: {}",
                    version.id, e
                );
            }
            if let Err(e) =
                ArtifactRepository::update_size_bytes(&self.inner.db, version.artifact, size).await
            {
                warn!(
                    "Failed to update size_bytes for sensor artifact {}: {}",
                    version.artifact, e
                );
            }
            synced_count += 1;
        }

        if synced_count > 0 {
            info!(
                "Synced {} local file-backed artifact version(s) for sensor '{}' to {} transport",
                synced_count,
                sensor.r#ref,
                self.inner.artifact_transport.transport_mode(),
            );
        }
    }

    async fn forget_sensor_instance(&self, sensor_id: Id) {
        self.inner.sensors.write().await.remove(&sensor_id);
    }

    /// Resolve a sensor's `runtime_version_constraint` against the locally
    /// available runtime versions for this sensor service instance.
    ///
    /// Returns `(execution_config_override, env_dir_suffix, version_string)`:
    /// - When the sensor declares no constraint AND the runtime has no
    ///   registered versions, all three are `None` and the parent runtime's
    ///   config is used as-is.
    /// - When the sensor declares a constraint and a matching version is
    ///   available locally, returns the version's config + per-version env
    ///   suffix (e.g., `"python-3.12"`).
    /// - When the sensor declares a constraint but no compatible version is
    ///   available locally, returns an error so this sensor service refuses
    ///   to start the sensor (allowing another sensor worker with a matching
    ///   version to pick it up).
    async fn resolve_runtime_version(
        &self,
        runtime: &attune_common::models::Runtime,
        sensor: &Sensor,
    ) -> Result<(
        Option<RuntimeExecutionConfig>,
        Option<String>,
        Option<String>,
    )> {
        let constraint = sensor.runtime_version_constraint.as_deref();

        // Load all registered versions for this runtime.
        let versions = RuntimeVersionRepository::find_by_runtime(&self.inner.db, runtime.id)
            .await
            .map_err(|e| {
                anyhow!(
                    "Failed to load runtime versions for runtime '{}' (id {}): {}",
                    runtime.name,
                    runtime.id,
                    e
                )
            })?;

        if versions.is_empty() {
            if constraint.is_some() {
                return Err(anyhow!(
                    "Sensor '{}' declares runtime_version_constraint '{}' but runtime '{}' \
                     has no registered versions; refusing to start.",
                    sensor.r#ref,
                    constraint.unwrap_or(""),
                    runtime.name,
                ));
            }
            return Ok((None, None, None));
        }

        // Filter to versions actually available on THIS sensor worker.
        let local_versions = self
            .filter_versions_for_local_worker(runtime, versions)
            .await;

        match select_best_version(&local_versions, constraint) {
            Some(selected) => {
                let version_config = selected.parsed_execution_config();
                let rt_name = runtime.name.to_lowercase();
                let env_suffix = format!("{}-{}", rt_name, selected.version);
                info!(
                    "Selected runtime version '{}' (id {}) for sensor '{}' \
                     (constraint: {}, runtime: '{}'). Env dir suffix: '{}'",
                    selected.version,
                    selected.id,
                    sensor.r#ref,
                    constraint.unwrap_or("none"),
                    runtime.name,
                    env_suffix,
                );
                Ok((
                    Some(version_config),
                    Some(env_suffix),
                    Some(selected.version.clone()),
                ))
            }
            None => {
                if let Some(c) = constraint {
                    Err(anyhow!(
                        "No locally available runtime version matches constraint '{}' for \
                         sensor '{}' (runtime: '{}'); refusing to start. Another sensor worker \
                         with a matching version may pick it up.",
                        c,
                        sensor.r#ref,
                        runtime.name,
                    ))
                } else {
                    debug!(
                        "No default or available version found for runtime '{}'. \
                         Using parent runtime config for sensor '{}'.",
                        runtime.name, sensor.r#ref,
                    );
                    Ok((None, None, None))
                }
            }
        }
    }

    /// Filter a list of registered runtime versions down to the ones actually
    /// present on this sensor worker, based on `worker.capabilities.runtime_versions`
    /// (populated at registration time from agent runtime detection).
    async fn filter_versions_for_local_worker(
        &self,
        runtime: &attune_common::models::Runtime,
        versions: Vec<attune_common::models::RuntimeVersion>,
    ) -> Vec<attune_common::models::RuntimeVersion> {
        let worker_id = self.inner.worker_id.load(Ordering::SeqCst);
        if worker_id == 0 {
            warn!(
                "Sensor manager has no worker_id assigned; cannot filter runtime versions for '{}'. \
                 Falling back to all registered versions.",
                runtime.name,
            );
            return versions;
        }

        let worker = match WorkerRepository::find_by_id(&self.inner.db, worker_id).await {
            Ok(Some(w)) => w,
            Ok(None) => {
                warn!(
                    "Sensor worker {} not found in DB while resolving runtime versions for '{}'; \
                     using all registered versions as fallback.",
                    worker_id, runtime.name,
                );
                return versions;
            }
            Err(e) => {
                warn!(
                    "Failed to load sensor worker {} while resolving runtime versions for '{}': {}. \
                     Using all registered versions as fallback.",
                    worker_id, runtime.name, e,
                );
                return versions;
            }
        };

        let advertised = Self::worker_runtime_versions_for_runtime(&worker, runtime);
        if advertised.is_empty() {
            warn!(
                "Sensor worker {} does not advertise local runtime versions for '{}'; \
                 using all registered versions as fallback.",
                worker.name, runtime.name,
            );
            return versions;
        }

        versions
            .into_iter()
            .filter(|v| Self::version_matches_worker(v, &advertised))
            .collect()
    }

    /// Extract the list of locally-detected version strings for `runtime`
    /// from a worker's `capabilities.runtime_versions` (or fall back to
    /// `capabilities.detected_interpreters`).
    fn worker_runtime_versions_for_runtime(
        worker: &attune_common::models::Worker,
        runtime: &attune_common::models::Runtime,
    ) -> Vec<String> {
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
            if let Some(map) = runtime_versions.as_object() {
                for runtime_name in &candidate_runtime_names {
                    if let Some(values) = map.get(runtime_name).and_then(|v| v.as_array()) {
                        versions.extend(
                            values
                                .iter()
                                .filter_map(|v| v.as_str().map(ToOwned::to_owned)),
                        );
                    }
                }
            }
        }

        if versions.is_empty() {
            if let Some(detected) = capabilities
                .get("detected_interpreters")
                .and_then(|v| v.as_array())
            {
                for interp in detected {
                    let Some(name) = interp.get("name").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if !candidate_runtime_names
                        .iter()
                        .any(|c| c == &normalize_runtime_name(name))
                    {
                        continue;
                    }
                    if let Some(version) = interp.get("version").and_then(|v| v.as_str()) {
                        versions.push(version.to_string());
                    }
                }
            }
        }

        versions
    }

    /// True if the registered `RuntimeVersion` matches one of the worker's
    /// advertised version strings (using the same lenient prefix match the
    /// action worker uses).
    fn version_matches_worker(
        version: &attune_common::models::RuntimeVersion,
        advertised: &[String],
    ) -> bool {
        use attune_common::version_matching::parse_version;

        let registered = match parse_version(&version.version) {
            Ok(v) => v,
            Err(_) => return false,
        };

        for adv in advertised {
            if adv == &version.version {
                return true;
            }
            if let Ok(adv_parsed) = parse_version(adv) {
                if adv_parsed == registered {
                    return true;
                }
                // Lenient: same major.minor counts as a match (worker reports
                // "3.12.1" while DB might have "3.12").
                if adv_parsed.major == registered.major && adv_parsed.minor == registered.minor {
                    return true;
                }
            }
        }
        false
    }

    /// Check if a sensor has active rules across any of its triggers
    async fn sensor_has_active_rules(&self, sensor_id: Id) -> Result<bool> {
        let count = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM rule r
            JOIN trigger t ON r.trigger = t.id
            WHERE t.sensor = $1
              AND r.enabled = TRUE
              AND t.enabled = TRUE
            "#,
        )
        .bind(sensor_id)
        .fetch_one(&self.inner.db)
        .await?;

        Ok(count > 0)
    }

    /// Get count of active rules across all triggers for a sensor
    async fn sensor_active_rule_count(&self, sensor_id: Id) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM rule r
            JOIN trigger t ON r.trigger = t.id
            WHERE t.sensor = $1
              AND r.enabled = TRUE
              AND t.enabled = TRUE
            "#,
        )
        .bind(sensor_id)
        .fetch_one(&self.inner.db)
        .await?;

        Ok(count)
    }

    /// Fetch trigger instances (enabled rules with their trigger params) for a trigger
    async fn fetch_trigger_instances(&self, trigger_id: Id) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"
            SELECT r.*, t.ref AS trigger_ref
            FROM rule r
            JOIN trigger t ON t.id = r.trigger
            WHERE r.trigger = $1
              AND r.enabled = TRUE
              AND t.enabled = TRUE
            "#,
        )
        .bind(trigger_id)
        .fetch_all(&self.inner.db)
        .await?;

        info!("Fetched {} rows from rule table", rows.len());

        // Convert to the format expected by timer sensor
        let trigger_instances: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|row| {
                let id: i64 = row.try_get("id").unwrap_or(0);
                let ref_str: String = row.try_get("ref").unwrap_or_default();
                let trigger_params: serde_json::Value = row
                    .try_get("trigger_params")
                    .unwrap_or(serde_json::json!({}));
                let trigger_ref: String = row.try_get("trigger_ref").unwrap_or_default();

                info!(
                    "Rule ID: {}, Ref: {}, Trigger: {}, Params: {}",
                    id, ref_str, trigger_ref, trigger_params
                );

                serde_json::json!({
                    "id": id,
                    "ref": ref_str,
                    "trigger_ref": trigger_ref,
                    "config": trigger_params
                })
            })
            .collect();

        Ok(trigger_instances)
    }

    /// Stop a sensor
    pub async fn stop_sensor(&self, sensor_id: Id) -> Result<()> {
        info!("Stopping sensor {}", sensor_id);

        let instance = {
            let mut sensors = self.inner.sensors.write().await;
            sensors.remove(&sensor_id)
        };

        if let Some(mut instance) = instance {
            let sensor = instance.sensor.clone();
            instance.stop().await;
            self.sync_sensor_file_artifacts(&sensor).await;
            self.persist_sensor_process_stopped(&sensor).await;
            info!("Sensor {} stopped", sensor_id);
        } else {
            warn!("Sensor {} not found in running instances", sensor_id);
        }

        Ok(())
    }

    async fn handle_unexpected_sensor_exit(&self, exited: ExitedSensorProcess) {
        let manager_running = *self.inner.running.read().await;
        {
            let mut status = exited.status.write().await;
            status.running = false;
            status.failed = true;
            status.failure_count = status.failure_count.saturating_add(1);
            status.last_poll = Some(Utc::now());
        }

        if !manager_running {
            debug!(
                "Sensor {} exited while manager is stopping; treating as intentional stop",
                exited.sensor.r#ref
            );
            self.sync_sensor_file_artifacts(&exited.sensor).await;
            self.persist_sensor_process_stopped(&exited.sensor).await;
            self.forget_sensor_instance(exited.sensor.id).await;
            return;
        }

        let active_rule_count = match self.sensor_active_rule_count(exited.sensor.id).await {
            Ok(count) => active_rule_count_i32(count),
            Err(e) => {
                warn!(
                    "Failed to count active rules after sensor {} exited: {}",
                    exited.sensor.r#ref, e
                );
                0
            }
        };

        if active_rule_count <= 0 {
            info!(
                "Sensor {} exited but has no active rules; marking stopped and not restarting",
                exited.sensor.r#ref
            );
            self.sync_sensor_file_artifacts(&exited.sensor).await;
            self.persist_sensor_process_stopped(&exited.sensor).await;
            self.forget_sensor_instance(exited.sensor.id).await;
            return;
        }

        let worker_id = self.inner.worker_id.load(Ordering::SeqCst);
        if worker_id <= 0 {
            warn!(
                "Sensor {} exited with active rules but worker_id is unset; cannot persist backoff or restart safely",
                exited.sensor.r#ref
            );
            self.forget_sensor_instance(exited.sensor.id).await;
            return;
        }

        let current_failure_count = SensorProcessRepository::find_by_sensor_and_worker(
            &self.inner.db,
            exited.sensor.id,
            worker_id,
        )
        .await
        .ok()
        .flatten()
        .map(|process| process.consecutive_failures.saturating_add(1))
        .unwrap_or(1);
        let backoff_delay = sensor_restart_backoff_delay(current_failure_count);
        let next_restart_at = Utc::now() + duration_to_chrono(backoff_delay);
        self.sync_sensor_file_artifacts(&exited.sensor).await;
        let stderr_excerpt = self.read_sensor_stderr_excerpt(&exited.sensor.r#ref).await;

        let process = match SensorProcessRepository::mark_failed_or_backoff(
            &self.inner.db,
            MarkSensorProcessFailedInput {
                sensor: exited.sensor.id,
                worker: worker_id,
                status: SensorProcessStatus::Backoff,
                exit_code: exited.exit_code,
                signal: exited.signal,
                stopped_at: Some(Utc::now()),
                stderr_excerpt: stderr_excerpt.clone(),
                active_rule_count,
                next_restart_at: Some(next_restart_at),
            },
        )
        .await
        {
            Ok(process) => process,
            Err(e) => {
                warn!(
                    "Failed to persist sensor_process backoff for {} on worker {}: {}",
                    exited.sensor.r#ref, worker_id, e
                );
                None
            }
        };

        if let Some(process) = process.as_ref() {
            self.maybe_emit_sensor_failure_alert(
                &exited.sensor,
                process,
                backoff_delay,
                stderr_excerpt.as_deref(),
            )
            .await;
        }

        self.schedule_sensor_restart(exited.sensor.id, backoff_delay);
    }

    fn schedule_sensor_restart(&self, sensor_id: Id, delay: Duration) {
        let manager = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            manager.attempt_sensor_restart(sensor_id).await;
        });
    }

    async fn attempt_sensor_restart(&self, sensor_id: Id) {
        if !*self.inner.running.read().await {
            debug!(
                "Skipping sensor {} restart because sensor manager is no longer running",
                sensor_id
            );
            return;
        }

        if !self.inner.sensors.read().await.contains_key(&sensor_id) {
            debug!(
                "Skipping sensor {} restart because no failed instance is awaiting restart",
                sensor_id
            );
            return;
        }

        if self.sensor_instance_running(sensor_id).await {
            debug!(
                "Skipping sensor {} restart because an instance is already running",
                sensor_id
            );
            return;
        }

        let sensor = match SensorRepository::find_by_id(&self.inner.db, sensor_id).await {
            Ok(Some(sensor)) if sensor.enabled => sensor,
            Ok(Some(sensor)) => {
                info!(
                    "Skipping restart for disabled sensor {} ({})",
                    sensor.r#ref, sensor.id
                );
                self.persist_sensor_process_stopped(&sensor).await;
                self.forget_sensor_instance(sensor.id).await;
                return;
            }
            Ok(None) => {
                info!(
                    "Skipping restart for deleted sensor {}; no sensor row exists",
                    sensor_id
                );
                self.forget_sensor_instance(sensor_id).await;
                return;
            }
            Err(e) => {
                warn!("Failed to load sensor {} for restart: {}", sensor_id, e);
                return;
            }
        };

        match self.sensor_active_rule_count(sensor.id).await {
            Ok(count) if count > 0 => {}
            Ok(_) => {
                info!(
                    "Skipping restart for sensor {} because it has no active rules",
                    sensor.r#ref
                );
                self.persist_sensor_process_stopped(&sensor).await;
                self.forget_sensor_instance(sensor.id).await;
                return;
            }
            Err(e) => {
                warn!(
                    "Skipping restart for sensor {} because active-rule count failed: {}",
                    sensor.r#ref, e
                );
                return;
            }
        }

        match self.sensor_matches_this_worker(&sensor).await {
            Ok(true) => {}
            Ok(false) => {
                info!(
                    "Skipping restart for sensor {} because placement no longer matches this worker",
                    sensor.r#ref
                );
                self.persist_sensor_process_stopped(&sensor).await;
                self.forget_sensor_instance(sensor.id).await;
                return;
            }
            Err(e) => {
                warn!(
                    "Skipping restart for sensor {} because placement evaluation failed: {}",
                    sensor.r#ref, e
                );
                return;
            }
        }

        if self.sensor_instance_running(sensor.id).await {
            return;
        }

        info!("Restarting sensor {} after backoff", sensor.r#ref);
        if let Err(e) = self.start_sensor(sensor.clone(), false).await {
            warn!("Failed to restart sensor {}: {}", sensor.r#ref, e);
            self.handle_sensor_restart_failure(sensor, e.to_string())
                .await;
        }
    }

    async fn handle_sensor_restart_failure(&self, sensor: Sensor, error_message: String) {
        let worker_id = self.inner.worker_id.load(Ordering::SeqCst);
        if worker_id <= 0 {
            warn!(
                "Cannot persist restart failure for sensor {} because worker_id is unset: {}",
                sensor.r#ref, error_message
            );
            return;
        }

        let active_rule_count = match self.sensor_active_rule_count(sensor.id).await {
            Ok(count) => active_rule_count_i32(count),
            Err(e) => {
                warn!(
                    "Failed to count active rules after restart failure for sensor {}: {}",
                    sensor.r#ref, e
                );
                0
            }
        };

        if active_rule_count <= 0 {
            self.persist_sensor_process_stopped(&sensor).await;
            self.forget_sensor_instance(sensor.id).await;
            return;
        }

        let next_failure_count = SensorProcessRepository::find_by_sensor_and_worker(
            &self.inner.db,
            sensor.id,
            worker_id,
        )
        .await
        .ok()
        .flatten()
        .map(|process| process.consecutive_failures.saturating_add(1))
        .unwrap_or(1);
        let backoff_delay = sensor_restart_backoff_delay(next_failure_count);
        let next_restart_at = Utc::now() + duration_to_chrono(backoff_delay);
        let stderr_excerpt = Some(format!("restart failed: {}", error_message));

        let process = match SensorProcessRepository::mark_failed_or_backoff(
            &self.inner.db,
            MarkSensorProcessFailedInput {
                sensor: sensor.id,
                worker: worker_id,
                status: SensorProcessStatus::Backoff,
                exit_code: None,
                signal: None,
                stopped_at: Some(Utc::now()),
                stderr_excerpt: stderr_excerpt.clone(),
                active_rule_count,
                next_restart_at: Some(next_restart_at),
            },
        )
        .await
        {
            Ok(process) => process,
            Err(e) => {
                warn!(
                    "Failed to persist restart backoff for sensor {} on worker {}: {}",
                    sensor.r#ref, worker_id, e
                );
                None
            }
        };

        if let Some(process) = process.as_ref() {
            self.maybe_emit_sensor_failure_alert(
                &sensor,
                process,
                backoff_delay,
                stderr_excerpt.as_deref(),
            )
            .await;
        }

        self.schedule_sensor_restart(sensor.id, backoff_delay);
    }

    async fn maybe_emit_sensor_failure_alert(
        &self,
        sensor: &Sensor,
        process: &SensorProcess,
        backoff_delay: Duration,
        stderr_excerpt: Option<&str>,
    ) {
        if process.active_rule_count <= 0
            || process.consecutive_failures < SENSOR_ALERT_FAILURE_THRESHOLD
            || process.last_alerted_failure_count >= process.consecutive_failures
        {
            return;
        }

        let alert = SystemAlert {
            severity: "error".to_string(),
            category: "sensor_process_health".to_string(),
            failure_type: "sensor_process_repeated_failure".to_string(),
            component_type: "sensor".to_string(),
            component_id: Some(sensor.id),
            component_ref: Some(sensor.r#ref.clone()),
            worker_role: Some("sensor".to_string()),
            observed_at: Utc::now(),
            summary: format!(
                "Sensor '{}' failed {} consecutive time(s); restarting after {}s backoff",
                sensor.r#ref,
                process.consecutive_failures,
                backoff_delay.as_secs()
            ),
            details: serde_json::json!({
                "sensor_id": sensor.id,
                "sensor_ref": sensor.r#ref,
                "worker_id": process.worker,
                "worker_name": process.worker_name,
                "process_status": process.status,
                "consecutive_failures": process.consecutive_failures,
                "last_exit_code": process.last_exit_code,
                "last_signal": process.last_signal,
                "backoff_delay_seconds": backoff_delay.as_secs(),
                "next_restart_at": process.next_restart_at,
                "active_rule_count": process.active_rule_count,
                "stderr_excerpt": stderr_excerpt,
            }),
            correlation_id: Some(format!(
                "sensor:{}:worker:{}:repeated_failure",
                sensor.id, process.worker
            )),
        };

        let emit_result = match Connection::connect(&self.inner.mq_url).await {
            Ok(connection) => match Publisher::new(
                &connection,
                PublisherConfig {
                    confirm_publish: true,
                    timeout_secs: 30,
                    exchange: "attune.events".to_string(),
                },
            )
            .await
            {
                Ok(publisher) => emit_core_alert(&self.inner.db, Some(&publisher), alert).await,
                Err(e) => {
                    warn!(
                        "Failed to create alert publisher for sensor {}: {}. Recording alert event without MQ publish.",
                        sensor.r#ref, e
                    );
                    emit_core_alert(&self.inner.db, None, alert).await
                }
            },
            Err(e) => {
                warn!(
                    "Failed to connect to MQ for sensor {} alert: {}. Recording alert event without MQ publish.",
                    sensor.r#ref, e
                );
                emit_core_alert(&self.inner.db, None, alert).await
            }
        };

        match emit_result {
            Ok(Some(_)) => {
                if let Err(e) = SensorProcessRepository::record_alerted(
                    &self.inner.db,
                    RecordSensorProcessAlertedInput {
                        sensor: sensor.id,
                        worker: process.worker,
                        failure_count: process.consecutive_failures,
                        alerted_at: Some(Utc::now()),
                    },
                )
                .await
                {
                    warn!(
                        "Failed to record sensor_process alert marker for sensor {} on worker {}: {}",
                        sensor.r#ref, process.worker, e
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!(
                    "Failed to emit repeated-failure alert for sensor {} on worker {}: {}",
                    sensor.r#ref, process.worker, e
                );
            }
        }
    }

    /// Handle rule changes (created, enabled, disabled)
    pub async fn handle_rule_change(&self, trigger_id: Id) -> Result<()> {
        info!("Handling rule change for trigger {}", trigger_id);

        // Load the trigger through the repository so column selection stays in
        // sync with the Trigger model as fields are added.
        let trigger = TriggerRepository::find_by_id(&self.inner.db, trigger_id).await?;

        let sensor_id = match trigger.and_then(|t| t.sensor) {
            Some(id) => id,
            None => {
                info!("Trigger {} has no associated sensor, skipping", trigger_id);
                return Ok(());
            }
        };

        // Load the sensor through the repository so column selection stays in
        // sync with the Sensor model.
        if let Some(sensor) = SensorRepository::find_by_id(&self.inner.db, sensor_id).await? {
            if !sensor.enabled {
                if self.sensor_instance_running(sensor.id).await {
                    info!("Stopping disabled sensor {}", sensor.r#ref);
                    if let Err(e) = self.stop_sensor(sensor.id).await {
                        error!("Failed to stop disabled sensor {}: {}", sensor.r#ref, e);
                    }
                }
                return Ok(());
            }

            // Check if sensor is actively running
            let is_running = self.sensor_instance_running(sensor.id).await;

            // Check if sensor should be running (has active rules across any trigger)
            let should_run = self.sensor_has_active_rules(sensor.id).await?;

            match (is_running, should_run) {
                (false, true) => {
                    if !self.sensor_matches_this_worker(&sensor).await? {
                        debug!(
                            "Skipping sensor {} because placement does not match this sensor worker",
                            sensor.r#ref
                        );
                        return Ok(());
                    }
                    // Start sensor
                    info!("Starting sensor {} due to rule change", sensor.r#ref);
                    if let Err(e) = self.start_sensor(sensor, true).await {
                        error!("Failed to start sensor: {}", e);
                    }
                }
                (true, false) => {
                    // Stop sensor
                    info!("Stopping sensor {} - no active rules", sensor.r#ref);
                    if let Err(e) = self.stop_sensor(sensor.id).await {
                        error!("Failed to stop sensor: {}", e);
                    }
                }
                (true, true) => {
                    if !self.sensor_matches_this_worker(&sensor).await? {
                        info!(
                            "Stopping sensor {} because placement no longer matches this sensor worker",
                            sensor.r#ref
                        );
                        if let Err(e) = self.stop_sensor(sensor.id).await {
                            error!("Failed to stop sensor after placement mismatch: {}", e);
                        }
                        return Ok(());
                    }
                    info!(
                        "Sensor {} already running; lifecycle message will update trigger instances in-process",
                        sensor.r#ref
                    );
                }
                (false, false) => {
                    // No action needed
                    debug!("Sensor {} - no action needed", sensor.r#ref);
                }
            }
        }

        Ok(())
    }

    /// Monitoring loop to check sensor health
    async fn monitoring_loop(&self) {
        let mut interval = interval(Duration::from_secs(60));

        while *self.inner.running.read().await {
            interval.tick().await;

            debug!("Sensor manager monitoring check");

            let exited = {
                let mut sensors = self.inner.sensors.write().await;
                let mut exited = Vec::new();

                for (sensor_id, instance) in sensors.iter_mut() {
                    if let Some(child) = instance.child_process.as_mut() {
                        match child.try_wait() {
                            Ok(Some(exit_status)) => {
                                warn!(
                                    "Sensor {} ({}) exited unexpectedly: {:?}",
                                    instance.sensor_ref, sensor_id, exit_status
                                );
                                if let Some(handle) = instance.stdout_handle.take() {
                                    handle.abort();
                                }
                                if let Some(handle) = instance.stderr_handle.take() {
                                    handle.abort();
                                }
                                instance.child_process = None;
                                exited.push(ExitedSensorProcess {
                                    sensor: instance.sensor.clone(),
                                    status: instance.status.clone(),
                                    exit_code: exit_status.code(),
                                    signal: exit_signal(&exit_status),
                                });
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(
                                    "Failed to poll sensor {} ({}) process status: {}",
                                    instance.sensor_ref, sensor_id, e
                                );
                            }
                        }
                    } else {
                        let status = instance.status.try_read();
                        if let Ok(status) = status {
                            if status.failed {
                                warn!(
                                    "Sensor {} has failed (failure_count: {})",
                                    sensor_id, status.failure_count
                                );
                            }
                        }
                    }
                }

                exited
            };

            for exited_process in exited {
                self.handle_unexpected_sensor_exit(exited_process).await;
            }
        }

        info!("Sensor manager monitoring loop stopped");
    }

    async fn lifecycle_reconciliation_loop(&self) {
        let mut interval = interval(Duration::from_secs(2));

        while *self.inner.running.read().await {
            interval.tick().await;

            if let Err(e) = self.reconcile_sensor_lifecycles().await {
                warn!("Sensor lifecycle reconciliation failed: {}", e);
            }
        }

        info!("Sensor lifecycle reconciliation loop stopped");
    }

    async fn reconcile_sensor_lifecycles(&self) -> Result<()> {
        let sensors = self.load_enabled_sensors().await?;

        for sensor in sensors {
            let is_running = self.sensor_instance_running(sensor.id).await;
            let should_run = self.sensor_has_active_rules(sensor.id).await?;

            match (is_running, should_run) {
                (false, true) => {
                    if !self.sensor_matches_this_worker(&sensor).await? {
                        debug!(
                            "Skipping reconciled start for sensor {} because placement does not match this sensor worker",
                            sensor.r#ref
                        );
                        continue;
                    }

                    info!(
                        "Starting sensor {} during lifecycle reconciliation",
                        sensor.r#ref
                    );
                    if let Err(e) = self.start_sensor(sensor, true).await {
                        error!("Failed to start sensor during reconciliation: {}", e);
                    }
                }
                (true, false) => {
                    info!(
                        "Stopping sensor {} during lifecycle reconciliation - no active rules",
                        sensor.r#ref
                    );
                    if let Err(e) = self.stop_sensor(sensor.id).await {
                        error!("Failed to stop sensor during reconciliation: {}", e);
                    }
                }
                (true, true) => {
                    if !self.sensor_matches_this_worker(&sensor).await? {
                        info!(
                            "Stopping sensor {} during lifecycle reconciliation because placement no longer matches",
                            sensor.r#ref
                        );
                        if let Err(e) = self.stop_sensor(sensor.id).await {
                            error!(
                                "Failed to stop placement-mismatched sensor during reconciliation: {}",
                                e
                            );
                        }
                    }
                }
                (false, false) => {}
            }
        }

        Ok(())
    }

    /// Handle a pack change event — restart any sensors belonging to this pack.
    ///
    /// Called when `pack.registered` fires, indicating that pack files may have
    /// been updated. Any running sensors for this pack are stopped and restarted
    /// so they pick up the new files.
    pub async fn handle_pack_change(&self, pack_ref: &str) -> Result<()> {
        info!("Handling pack change for pack '{}'", pack_ref);

        let sensors = self.inner.sensors.read().await;
        let affected_ids: Vec<Id> = sensors
            .iter()
            .filter(|(_, inst)| inst.sensor.pack_ref.as_deref() == Some(pack_ref))
            .map(|(id, _)| *id)
            .collect();
        drop(sensors);

        if affected_ids.is_empty() {
            info!(
                "No running sensors for pack '{}', nothing to restart",
                pack_ref
            );
            return Ok(());
        }

        info!(
            "Restarting {} sensor(s) for updated pack '{}'",
            affected_ids.len(),
            pack_ref,
        );

        for sensor_id in &affected_ids {
            if let Err(e) = self.stop_sensor(*sensor_id).await {
                warn!(
                    "Failed to stop sensor {} for pack restart: {}",
                    sensor_id, e
                );
            }
        }

        // Re-trigger reconciliation to restart sensors from the database
        self.reconcile_sensors().await;

        Ok(())
    }

    /// Handle a pack deleted event — stop any sensors belonging to this pack.
    pub async fn handle_pack_deleted(&self, pack_ref: &str) -> Result<()> {
        info!("Handling pack deletion for pack '{}'", pack_ref);

        let sensors = self.inner.sensors.read().await;
        let affected_ids: Vec<Id> = sensors
            .iter()
            .filter(|(_, inst)| inst.sensor.pack_ref.as_deref() == Some(pack_ref))
            .map(|(id, _)| *id)
            .collect();
        drop(sensors);

        if affected_ids.is_empty() {
            info!("No running sensors for deleted pack '{}'", pack_ref);
            return Ok(());
        }

        info!(
            "Stopping {} sensor(s) for deleted pack '{}'",
            affected_ids.len(),
            pack_ref,
        );

        for sensor_id in &affected_ids {
            if let Err(e) = self.stop_sensor(*sensor_id).await {
                warn!(
                    "Failed to stop sensor {} for deleted pack '{}': {}",
                    sensor_id, pack_ref, e,
                );
            }
        }

        Ok(())
    }

    /// Reconcile the running sensor set against the database.
    ///
    /// Loads all enabled sensors with active rules and starts any that
    /// are not already running.
    async fn reconcile_sensors(&self) {
        let sensors = match self.load_enabled_sensors().await {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to load sensors for reconciliation: {}", e);
                return;
            }
        };

        for sensor in sensors {
            // Skip sensors already running
            if self.sensor_instance_running(sensor.id).await {
                continue;
            }

            match self.sensor_has_active_rules(sensor.id).await {
                Ok(true) => {
                    match self.sensor_matches_this_worker(&sensor).await {
                        Ok(true) => {}
                        Ok(false) => {
                            debug!(
                                "Reconcile: skipping sensor {} because placement does not match this sensor worker",
                                sensor.r#ref
                            );
                            continue;
                        }
                        Err(e) => {
                            warn!(
                                "Reconcile: failed to evaluate placement for sensor {}: {}",
                                sensor.r#ref, e
                            );
                            continue;
                        }
                    }
                    info!("Reconcile: starting sensor {}", sensor.r#ref);
                    if let Err(e) = self.start_sensor(sensor, true).await {
                        warn!("Reconcile: failed to start sensor: {}", e);
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        "Reconcile: failed to check active rules for sensor {}: {}",
                        sensor.r#ref, e,
                    );
                }
            }
        }
    }

    async fn sensor_matches_this_worker(&self, sensor: &Sensor) -> Result<bool> {
        let worker_id = self.inner.worker_id.load(Ordering::SeqCst);
        if worker_id <= 0 {
            return Ok(false);
        }
        let Some(worker) = WorkerRepository::find_by_id(&self.inner.db, worker_id).await? else {
            return Ok(false);
        };

        let labels = worker_labels_from_capabilities(worker.capabilities.as_ref());
        let taints = worker_taints_from_capabilities(worker.capabilities.as_ref());
        Ok(worker_matches_placement(
            &labels,
            &taints,
            &sensor.worker_selector_labels(),
            &sensor.worker_toleration_specs(),
            &sensor.worker_affinity_spec(),
        ))
    }

    /// Get count of active sensors
    pub async fn active_count(&self) -> usize {
        let sensors = self.inner.sensors.read().await;
        let mut active = 0;

        for instance in sensors.values() {
            let status = instance.status().await;
            if status.running && !status.failed {
                active += 1;
            }
        }

        active
    }

    /// Get count of failed sensors
    pub async fn failed_count(&self) -> usize {
        let sensors = self.inner.sensors.read().await;
        let mut failed = 0;

        for instance in sensors.values() {
            let status = instance.status().await;
            if status.failed {
                failed += 1;
            }
        }

        failed
    }

    /// Get total count of sensors
    pub async fn total_count(&self) -> usize {
        self.inner.sensors.read().await.len()
    }

    /// Get sensor activity metrics suitable for worker heartbeat/capability reporting.
    pub async fn activity_metrics(&self) -> Result<SensorActivityMetrics> {
        let running_sensor_ids: Vec<Id> = self.inner.sensors.read().await.keys().copied().collect();
        let running_sensors = self.active_count().await as u64;

        if running_sensor_ids.is_empty() {
            return Ok(SensorActivityMetrics {
                monitored_sensors: 0,
                running_sensors,
                active_rules: 0,
            });
        }

        let active_rules = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(DISTINCT rule.id)
            FROM rule
            JOIN trigger ON trigger.id = rule.trigger
            WHERE trigger.sensor = ANY($1)
              AND rule.enabled = TRUE
              AND trigger.enabled = TRUE
            "#,
        )
        .bind(&running_sensor_ids)
        .fetch_one(&self.inner.db)
        .await?;

        Ok(SensorActivityMetrics {
            monitored_sensors: running_sensor_ids.len() as u64,
            running_sensors,
            active_rules: active_rules.max(0) as u64,
        })
    }
}

/// Sensor instance managing a running sensor
struct SensorInstance {
    sensor: Sensor,
    sensor_ref: String,
    status: Arc<RwLock<SensorStatus>>,
    child_process: Option<Child>,
    stderr_handle: Option<JoinHandle<()>>,
    stdout_handle: Option<JoinHandle<()>>,
}

impl SensorInstance {
    /// Create a new standalone sensor instance
    fn new_standalone(
        sensor: Sensor,
        child_process: Child,
        stdout_handle: JoinHandle<()>,
        stderr_handle: JoinHandle<()>,
    ) -> Self {
        let sensor_ref = sensor.r#ref.clone();
        Self {
            sensor,
            sensor_ref,
            status: Arc::new(RwLock::new(SensorStatus {
                running: true,
                failed: false,
                failure_count: 0,
                last_poll: Some(chrono::Utc::now()),
            })),
            child_process: Some(child_process),
            stderr_handle: Some(stderr_handle),
            stdout_handle: Some(stdout_handle),
        }
    }

    /// Stop the sensor
    async fn stop(&mut self) {
        {
            let mut status = self.status.write().await;
            status.running = false;
        }

        // Kill child process if exists
        if let Some(ref mut child) = self.child_process {
            terminate_sensor_child(child, &self.sensor_ref).await;
        }

        // Abort task handles
        if let Some(ref handle) = self.stdout_handle {
            handle.abort();
        }

        if let Some(ref handle) = self.stderr_handle {
            handle.abort();
        }
    }

    /// Get sensor status
    async fn status(&self) -> SensorStatus {
        self.status.read().await.clone()
    }
}

/// Sensor status information
#[derive(Clone, Debug, Default)]
pub struct SensorStatus {
    /// Whether the sensor is running
    pub running: bool,

    /// Whether the sensor has failed
    pub failed: bool,

    /// Number of consecutive failures
    pub failure_count: u32,

    /// Last successful poll time
    pub last_poll: Option<chrono::DateTime<chrono::Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::models::enums::ActionReferenceVisibility;
    use attune_common::models::runtime::{
        RuntimeEnvVarConfig, RuntimeEnvVarOperation, RuntimeEnvVarSpec,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::fs;
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn test_workspace_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/sensor-manager-tests")
            .join(format!("{}-{}-{}", name, std::process::id(), unique))
    }

    fn test_trigger(trigger_ref: &str, enabled: bool) -> Trigger {
        Trigger {
            id: 1,
            r#ref: trigger_ref.to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            label: "Trigger".to_string(),
            description: None,
            enabled,
            param_schema: None,
            out_schema: None,
            webhook_enabled: false,
            webhook_key: None,
            webhook_config: None,
            sensor: Some(1),
            sensor_ref: Some("core.timer_sensor".to_string()),
            is_adhoc: false,
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_sensor_status_default() {
        let status = SensorStatus::default();
        assert!(!status.running);
        assert!(!status.failed);
        assert_eq!(status.failure_count, 0);
        assert!(status.last_poll.is_none());
    }

    #[test]
    fn test_sensor_restart_backoff_delay_sequence_and_cap() {
        let cases = [
            (-1, 5),
            (0, 5),
            (1, 5),
            (2, 10),
            (3, 20),
            (4, 40),
            (5, 80),
            (6, 160),
            (7, 300),
            (8, 300),
            (100, 300),
        ];

        for (failure_count, expected_secs) in cases {
            assert_eq!(
                sensor_restart_backoff_delay(failure_count),
                Duration::from_secs(expected_secs),
                "failure_count={failure_count}"
            );
        }
    }

    #[test]
    fn test_active_rule_count_i32_clamps_values() {
        assert_eq!(active_rule_count_i32(-1), 0);
        assert_eq!(active_rule_count_i32(0), 0);
        assert_eq!(active_rule_count_i32(42), 42);
        assert_eq!(active_rule_count_i32(i32::MAX as i64), i32::MAX);
        assert_eq!(active_rule_count_i32(i32::MAX as i64 + 1), i32::MAX);
        assert_eq!(active_rule_count_i32(i64::MAX), i32::MAX);
    }

    #[test]
    fn test_collect_sensor_token_trigger_types_includes_disabled_and_deduplicates() {
        let triggers = vec![
            test_trigger("core.intervaltimer", true),
            test_trigger("core.crontimer", false),
            test_trigger(" core.intervaltimer ", false),
            test_trigger("", true),
        ];

        assert_eq!(
            collect_sensor_token_trigger_types(&triggers),
            vec![
                "core.crontimer".to_string(),
                "core.intervaltimer".to_string()
            ]
        );
    }

    #[test]
    fn test_duration_to_chrono_converts_and_falls_back_on_overflow() {
        assert_eq!(
            duration_to_chrono(Duration::from_millis(1_500)),
            chrono::Duration::milliseconds(1_500)
        );
        assert_eq!(
            duration_to_chrono(Duration::MAX),
            chrono::Duration::seconds(SENSOR_RESTART_MAX_DELAY.as_secs() as i64)
        );
    }

    #[tokio::test]
    async fn test_read_sensor_stderr_excerpt_returns_none_for_missing_or_empty_logs() {
        let artifacts_dir = test_workspace_path("stderr-empty");
        let sensor_dir = artifacts_dir.join("sensors").join("core.test_sensor");
        fs::create_dir_all(&sensor_dir).await.unwrap();

        assert_eq!(
            read_sensor_stderr_excerpt_from_artifacts_dir(&artifacts_dir, "missing.sensor").await,
            None
        );

        fs::write(sensor_dir.join("stderr.log"), "\n\n")
            .await
            .unwrap();
        assert_eq!(
            read_sensor_stderr_excerpt_from_artifacts_dir(&artifacts_dir, "core.test_sensor").await,
            None
        );

        fs::remove_dir_all(&artifacts_dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_read_sensor_stderr_excerpt_keeps_recent_lines() {
        let artifacts_dir = test_workspace_path("stderr-lines");
        let sensor_dir = artifacts_dir.join("sensors").join("core.test_sensor");
        fs::create_dir_all(&sensor_dir).await.unwrap();
        let log = (1..=100)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(sensor_dir.join("stderr.log"), log).await.unwrap();

        let excerpt =
            read_sensor_stderr_excerpt_from_artifacts_dir(&artifacts_dir, "core.test_sensor")
                .await
                .expect("stderr excerpt should be available");
        let lines = excerpt.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), STDERR_EXCERPT_MAX_LINES);
        assert_eq!(lines.first(), Some(&"line-21"));
        assert_eq!(lines.last(), Some(&"line-100"));
        assert!(!excerpt.starts_with('…'));

        fs::remove_dir_all(&artifacts_dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_read_sensor_stderr_excerpt_marks_truncated_byte_window() {
        let artifacts_dir = test_workspace_path("stderr-truncated");
        let sensor_dir = artifacts_dir.join("sensors").join("core.test_sensor");
        fs::create_dir_all(&sensor_dir).await.unwrap();
        let log = (1..=200)
            .map(|i| format!("line-{i:03}: {}", "x".repeat(120)))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(sensor_dir.join("stderr.log"), log).await.unwrap();

        let excerpt =
            read_sensor_stderr_excerpt_from_artifacts_dir(&artifacts_dir, "core.test_sensor")
                .await
                .expect("stderr excerpt should be available");
        let lines = excerpt.lines().collect::<Vec<_>>();

        assert!(excerpt.starts_with("…\n"));
        assert!(lines.len() <= STDERR_EXCERPT_MAX_LINES + 1);
        let expected_last_line = format!("line-200: {}", "x".repeat(120));
        assert_eq!(lines.last().copied(), Some(expected_last_line.as_str()));

        fs::remove_dir_all(&artifacts_dir).await.unwrap();
    }

    #[test]
    fn test_apply_runtime_env_vars_prepends_to_existing_command_env() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "PYTHONPATH".to_string(),
            RuntimeEnvVarConfig::Spec(RuntimeEnvVarSpec {
                value: "{pack_dir}/lib".to_string(),
                operation: RuntimeEnvVarOperation::Prepend,
                separator: ":".to_string(),
            }),
        );

        let exec_config = RuntimeExecutionConfig {
            env_vars,
            ..RuntimeExecutionConfig::default()
        };

        let mut cmd = Command::new("python3");
        cmd.env("PYTHONPATH", "/existing/pythonpath");

        apply_runtime_env_vars(
            &mut cmd,
            &exec_config,
            std::path::Path::new("/packs/testpack"),
            None,
        );

        let resolved = cmd
            .as_std()
            .get_envs()
            .find_map(|(key, value)| {
                if key == "PYTHONPATH" {
                    value.map(|value| value.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .expect("PYTHONPATH should be set");

        assert_eq!(resolved, "/packs/testpack/lib:/existing/pythonpath");
    }

    #[tokio::test]
    async fn test_sensor_instance_stop_reaps_child_process() {
        let test_dir = test_workspace_path("sensor-stop");
        fs::create_dir_all(&test_dir).await.unwrap();
        let script = test_dir.join("sensor-stop.sh");
        fs::write(&script, "#!/bin/sh\ntrap '' TERM\nsleep 30 &\nwait\n")
            .await
            .unwrap();

        let mut perms = fs::metadata(&script).await.unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        fs::set_permissions(&script, perms).await.unwrap();

        let mut cmd = Command::new("/bin/sh");
        cmd.arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_sensor_process(&mut cmd).unwrap();

        let mut child = cmd.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let stdout_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(_)) = reader.next_line().await {}
        });
        let stderr_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(_)) = reader.next_line().await {}
        });

        let test_sensor = attune_common::models::Sensor {
            id: 0,
            r#ref: "test.sensor".to_string(),
            pack: None,
            pack_ref: None,
            label: "Test Sensor".to_string(),
            description: None,
            entrypoint: "test.sh".to_string(),
            runtime: 0,
            runtime_ref: "core.shell".to_string(),
            runtime_version_constraint: None,
            enabled: true,
            param_schema: None,
            config: None,
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        };

        let mut instance =
            SensorInstance::new_standalone(test_sensor, child, stdout_handle, stderr_handle);
        instance.stop().await;

        let status = instance.child_process.as_mut().unwrap().try_wait().unwrap();
        assert!(
            status.is_some(),
            "child process should be reaped after stop()"
        );

        fs::remove_dir_all(&test_dir).await.unwrap();
    }
}
