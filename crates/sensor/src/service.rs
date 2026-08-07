//! Sensor Service
//!
//! Main service orchestrator that coordinates sensor management
//! and rule lifecycle listening.
//!
//! Shutdown follows the same pattern as the worker service:
//! 1. Deregister worker (mark inactive, stop receiving new work)
//! 2. Stop heartbeat
//! 3. Stop sensor processes with configurable timeout
//! 4. Close MQ and DB connections

use crate::rule_lifecycle_listener::RuleLifecycleListener;
use crate::sensor_manager::{SensorManager, SensorManagerConfig};
use crate::sensor_worker_registration::SensorWorkerRegistration;
use anyhow::Result;
use attune_common::agent_runtime_detection::DetectedRuntime;
use attune_common::auth::WorkerTokenProvider;
use attune_common::config::Config;
use attune_common::db::Database;
use attune_common::mq::MessageQueue;
use attune_common::repositories::List;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Sensor Service state
#[derive(Clone)]
pub struct SensorService {
    inner: Arc<SensorServiceInner>,
}

struct SensorServiceInner {
    config: Config,
    db: PgPool,
    mq: MessageQueue,
    sensor_manager: Arc<SensorManager>,
    rule_lifecycle_listener: Arc<RuleLifecycleListener>,
    sensor_worker_registration: Arc<RwLock<SensorWorkerRegistration>>,
    pack_transport: Arc<dyn attune_common::pack_transport::PackFileTransport>,
    heartbeat_interval: u64,
    heartbeat_running: Arc<RwLock<bool>>,
    detected_runtimes: RwLock<Option<Vec<DetectedRuntime>>>,
}

impl SensorService {
    async fn sync_worker_metrics(&self) -> Result<()> {
        let metrics = self.inner.sensor_manager.activity_metrics().await?;
        let mut registration = self.inner.sensor_worker_registration.write().await;
        let changed_monitored = registration.add_capability(
            "sensor_processes_monitored".to_string(),
            json!(metrics.monitored_sensors),
        );
        let changed_running = registration.add_capability(
            "sensor_processes_running".to_string(),
            json!(metrics.running_sensors),
        );
        let changed_rules =
            registration.add_capability("active_rules".to_string(), json!(metrics.active_rules));
        if changed_monitored || changed_running || changed_rules {
            registration.update_capabilities().await?;
        }
        Ok(())
    }

    /// Create a new sensor service
    pub async fn new(config: Config) -> Result<Self> {
        info!("Initializing Sensor Service");

        // Connect to database
        info!("Connecting to database...");
        let database = Database::new(&config.database).await?;
        let db = database.pool().clone();
        info!("Database connection established");

        // Connect to message queue
        info!("Connecting to message queue...");
        let mq_config = config
            .message_queue
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Message queue configuration is required"))?;
        let mq = MessageQueue::connect(&mq_config.url).await?;
        info!("Message queue connection established");

        // Setup common message queue infrastructure (exchanges and DLX)
        let mq_setup_config = attune_common::mq::MessageQueueConfig::default();
        match mq
            .get_connection()
            .setup_common_infrastructure(&mq_setup_config)
            .await
        {
            Ok(_) => info!("Common message queue infrastructure setup completed"),
            Err(e) => {
                warn!(
                    "Failed to setup common MQ infrastructure (may already exist): {}",
                    e
                );
            }
        }

        // Setup sensor-specific queues and bindings
        match mq
            .get_connection()
            .setup_sensor_infrastructure(&mq_setup_config)
            .await
        {
            Ok(_) => info!("Sensor message queue infrastructure setup completed"),
            Err(e) => {
                warn!(
                    "Failed to setup sensor MQ infrastructure (may already exist): {}",
                    e
                );
            }
        }

        // Create service components
        info!("Creating service components...");

        // Initialize pack file transport
        let api_url = std::env::var("ATTUNE_API_URL")
            .unwrap_or_else(|_| format!("http://{}:{}", config.server.host, config.server.port));
        let mq_url = std::env::var("ATTUNE_MQ_URL").unwrap_or_else(|_| {
            config
                .message_queue
                .as_ref()
                .map(|mq| mq.url.clone())
                .unwrap_or_else(|| "amqp://guest:guest@127.0.0.1:5672".to_string())
        });

        let jwt_config = attune_common::auth::jwt::JwtConfig {
            secret: config
                .security
                .jwt_secret
                .clone()
                .unwrap_or_else(|| "insecure_default_secret_change_in_production".to_string()),
            access_token_expiration: config.security.jwt_access_expiration as i64,
            refresh_token_expiration: config.security.jwt_refresh_expiration as i64,
        };

        let worker_token_provider = Arc::new(WorkerTokenProvider::new(
            1,
            format!("sensor-{}", uuid::Uuid::new_v4()),
            jwt_config,
        ));

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

        let artifact_transport: Arc<dyn attune_common::artifact_transport::ArtifactFileTransport> = {
            let transport =
                attune_common::artifact_transport::build_transport_with_worker_token_provider(
                    &config.artifacts_dir,
                    Some(&api_url),
                    Some(worker_token_provider.clone()),
                    &config.artifacts.transport,
                );
            info!(
                "Artifact file transport initialized for sensor logs: mode={}",
                transport.transport_mode()
            );
            Arc::from(transport)
        };

        let default_sensor_log_config = crate::sensor_log::SensorLogConfig::default();
        let sensor_log_config = crate::sensor_log::SensorLogConfig {
            max_bytes: config.artifacts.sensor_log_max_bytes,
            max_files: config.artifacts.sensor_log_max_files,
            ..default_sensor_log_config
        };

        let notifier_ws_url = resolve_notifier_ws_url(&config).await?;
        let allow_insecure_notifier_ws = config
            .sensor
            .as_ref()
            .map(|sensor| sensor.allow_insecure_notifier_ws)
            .unwrap_or(false);

        let sensor_manager = Arc::new(SensorManager::new(
            db.clone(),
            SensorManagerConfig {
                api_url,
                worker_token_provider: Some(worker_token_provider),
                notifier_ws_url,
                allow_insecure_notifier_ws,
                mq_url,
                log_level: config.log.level.clone(),
                log_format: config.log.format.clone(),
                packs_base_dir: config.packs_base_dir.clone(),
                runtime_envs_dir: config.runtime_envs_dir.clone(),
                artifact_transport,
                sensor_log_config,
            },
        ));

        // Create rule lifecycle listener
        let rule_lifecycle_listener = Arc::new(RuleLifecycleListener::new(
            db.clone(),
            mq.get_connection().clone(),
            sensor_manager.clone(),
            pack_transport.clone(),
        ));

        // Create sensor worker registration
        let sensor_worker_registration = SensorWorkerRegistration::new(db.clone(), &config);
        let heartbeat_interval = config
            .sensor
            .as_ref()
            .map(|s| s.heartbeat_interval)
            .unwrap_or(30);

        Ok(Self {
            inner: Arc::new(SensorServiceInner {
                config,
                db,
                mq,
                sensor_manager,
                rule_lifecycle_listener,
                sensor_worker_registration: Arc::new(RwLock::new(sensor_worker_registration)),
                pack_transport,
                heartbeat_interval,
                heartbeat_running: Arc::new(RwLock::new(false)),
                detected_runtimes: RwLock::new(None),
            }),
        })
    }

    /// Pass agent-detected runtimes (with versions) to be stored in worker
    /// capabilities at registration time. Used by `attune-sensor-agent`.
    pub async fn with_detected_runtimes(self, runtimes: Vec<DetectedRuntime>) -> Self {
        *self.inner.detected_runtimes.write().await = Some(runtimes);
        self
    }

    /// Start the sensor service
    ///
    /// Spawns background tasks (heartbeat, rule listener, sensor manager) and returns.
    /// The caller is responsible for blocking on shutdown signals and calling `stop()`.
    pub async fn start(&self) -> Result<()> {
        info!("Starting Sensor Service");

        // Register sensor worker
        info!("Registering sensor worker...");
        // If running as agent with auto-detected runtimes, push them into
        // capabilities BEFORE registration so they survive into the DB.
        let detected = self.inner.detected_runtimes.write().await.take();
        if let Some(detected) = detected {
            let mut reg = self.inner.sensor_worker_registration.write().await;
            info!(
                "Sensor agent mode: storing {} detected interpreter(s) in capabilities",
                detected.len()
            );
            reg.set_detected_runtimes(detected);
            reg.set_agent_mode(true);
        }
        let worker_id = self
            .inner
            .sensor_worker_registration
            .write()
            .await
            .register(&self.inner.config)
            .await?;
        info!("Sensor worker registered with ID: {}", worker_id);
        self.inner.sensor_manager.set_worker_id(worker_id);

        // Sync pack files from API if not using a shared volume
        self.sync_all_packs_on_startup().await;

        // Start rule lifecycle listener
        info!("Starting rule lifecycle listener...");
        if let Err(e) = self.inner.rule_lifecycle_listener.start().await {
            error!("Failed to start rule lifecycle listener: {}", e);
            return Err(e);
        }
        info!("Rule lifecycle listener started");

        // Start sensor manager
        info!("Starting sensor manager...");
        if let Err(e) = self.inner.sensor_manager.start().await {
            error!("Failed to start sensor manager: {}", e);
            return Err(e);
        }
        info!("Sensor manager started");

        if let Err(e) = self.sync_worker_metrics().await {
            warn!("Failed to sync initial sensor worker metrics: {}", e);
        }

        // Start heartbeat loop
        *self.inner.heartbeat_running.write().await = true;

        let sensor_manager = self.inner.sensor_manager.clone();
        let registration = self.inner.sensor_worker_registration.clone();
        let heartbeat_interval = self.inner.heartbeat_interval;
        let heartbeat_running = self.inner.heartbeat_running.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(heartbeat_interval));

            loop {
                ticker.tick().await;

                if !*heartbeat_running.read().await {
                    info!("Heartbeat loop stopping");
                    break;
                }

                match sensor_manager.activity_metrics().await {
                    Ok(metrics) => {
                        let mut guard = registration.write().await;
                        let changed_monitored = guard.add_capability(
                            "sensor_processes_monitored".to_string(),
                            json!(metrics.monitored_sensors),
                        );
                        let changed_running = guard.add_capability(
                            "sensor_processes_running".to_string(),
                            json!(metrics.running_sensors),
                        );
                        let changed_rules = guard.add_capability(
                            "active_rules".to_string(),
                            json!(metrics.active_rules),
                        );
                        if changed_monitored || changed_running || changed_rules {
                            if let Err(e) = guard.update_capabilities().await {
                                error!("Failed to update sensor worker metrics: {}", e);
                            }
                        }
                    }
                    Err(e) => error!("Failed to collect sensor worker metrics: {}", e),
                }

                if let Err(e) = registration.read().await.heartbeat().await {
                    error!("Failed to send sensor worker heartbeat: {}", e);
                }
            }

            info!("Heartbeat loop stopped");
        });

        info!("Sensor Service started successfully");

        Ok(())
    }

    /// Sync all registered packs from the API if using API-based pack transport.
    async fn sync_all_packs_on_startup(&self) {
        let transport = &self.inner.pack_transport;
        if transport.transport_mode() == "volume" {
            tracing::debug!("Pack transport is volume-based — skipping startup sync");
            return;
        }

        info!(
            "Syncing all packs via {} transport...",
            transport.transport_mode()
        );

        let packs = match attune_common::repositories::PackRepository::list(&self.inner.db).await {
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
            if transport.is_pack_local(&pack.r#ref).await {
                skipped += 1;
                continue;
            }

            match transport.sync_pack(&pack.r#ref).await {
                Ok(()) => {
                    synced += 1;
                    tracing::debug!("Synced pack '{}'", pack.r#ref);
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

    /// Stop the sensor service gracefully
    ///
    /// Shutdown order (mirrors worker service pattern):
    /// 1. Deregister worker (mark inactive to stop being scheduled for new work)
    /// 2. Stop heartbeat
    /// 3. Stop sensor processes with timeout
    /// 4. Stop rule lifecycle listener
    /// 5. Close MQ and DB connections
    pub async fn stop(&self) -> Result<()> {
        info!("Stopping Sensor Service - initiating graceful shutdown");

        // 1. Deregister sensor worker first to stop receiving new work
        info!("Marking sensor worker as inactive to stop receiving new work");
        if let Err(e) = self
            .inner
            .sensor_worker_registration
            .read()
            .await
            .deregister()
            .await
        {
            error!("Failed to deregister sensor worker: {}", e);
        }

        // 2. Stop heartbeat
        info!("Stopping heartbeat updates");
        *self.inner.heartbeat_running.write().await = false;

        // Wait a bit for heartbeat loop to notice the flag
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 3. Stop sensor processes with timeout
        let shutdown_timeout = self
            .inner
            .config
            .sensor
            .as_ref()
            .map(|s| s.shutdown_timeout)
            .unwrap_or(30);

        info!(
            "Waiting up to {} seconds for sensor processes to stop",
            shutdown_timeout
        );

        let sensor_manager = self.inner.sensor_manager.clone();
        let timeout_duration = Duration::from_secs(shutdown_timeout);
        match tokio::time::timeout(timeout_duration, sensor_manager.stop()).await {
            Ok(Ok(_)) => info!("All sensor processes stopped"),
            Ok(Err(e)) => error!("Error stopping sensor processes: {}", e),
            Err(_) => warn!(
                "Shutdown timeout reached ({} seconds) - some sensor processes may have been interrupted",
                shutdown_timeout
            ),
        }

        // 4. Stop rule lifecycle listener
        info!("Stopping rule lifecycle listener...");
        if let Err(e) = self.inner.rule_lifecycle_listener.stop().await {
            error!("Failed to stop rule lifecycle listener: {}", e);
        }

        // 5. Close message queue connection
        info!("Closing message queue connection...");
        if let Err(e) = self.inner.mq.close().await {
            warn!("Error closing message queue: {}", e);
        }

        // 6. Close database connection
        info!("Closing database connection...");
        self.inner.db.close().await;

        info!("Sensor Service stopped successfully");

        Ok(())
    }

    /// Get database pool
    pub fn db(&self) -> &PgPool {
        &self.inner.db
    }

    /// Get message queue
    pub fn mq(&self) -> &MessageQueue {
        &self.inner.mq
    }

    /// Get sensor manager
    pub fn sensor_manager(&self) -> Arc<SensorManager> {
        self.inner.sensor_manager.clone()
    }

    /// Get health status
    pub async fn health_check(&self) -> HealthStatus {
        // Check database connection
        if let Err(e) = sqlx::query("SELECT 1").execute(&self.inner.db).await {
            return HealthStatus::Unhealthy(format!("Database connection failed: {}", e));
        }

        // Check sensor manager health
        let active_sensors = self.inner.sensor_manager.active_count().await;
        let failed_sensors = self.inner.sensor_manager.failed_count().await;

        if active_sensors == 0 {
            return HealthStatus::Degraded("No active sensors".to_string());
        }

        if failed_sensors > 10 {
            return HealthStatus::Degraded(format!("{} sensors have failed", failed_sensors));
        }

        HealthStatus::Healthy
    }
}

/// Health status enumeration
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    /// Service is healthy
    Healthy,
    /// Service is degraded but operational
    Degraded(String),
    /// Service is unhealthy
    Unhealthy(String),
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded(msg) => write!(f, "degraded: {}", msg),
            HealthStatus::Unhealthy(msg) => write!(f, "unhealthy: {}", msg),
        }
    }
}

async fn resolve_notifier_ws_url(config: &Config) -> Result<String> {
    let sensor_config = config.sensor.as_ref();
    let configured_url = select_notifier_ws_url(
        config,
        std::env::var("ATTUNE_NOTIFIER_WS_URL")
            .ok()
            .filter(|url| !url.trim().is_empty()),
    );

    validate_notifier_ws_url(
        configured_url.trim(),
        sensor_config
            .map(|sensor| sensor.allow_insecure_notifier_ws)
            .unwrap_or(false),
    )
    .await
}

fn select_notifier_ws_url(config: &Config, legacy_url: Option<String>) -> String {
    legacy_url
        .or_else(|| {
            config
                .sensor
                .as_ref()
                .and_then(|sensor| sensor.notifier_ws_url.clone())
        })
        // The bind address is not a client endpoint. This fallback is only for
        // a notifier running on the same host as the sensor.
        .unwrap_or_else(|| "ws://127.0.0.1:8081/ws".to_string()) // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Intentional loopback-only local fallback.
}

async fn validate_notifier_ws_url(value: &str, allow_insecure: bool) -> Result<String> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| anyhow::anyhow!("invalid sensor notifier websocket URL: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("sensor notifier websocket URL must not contain credentials");
    }

    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("sensor notifier websocket URL must include a host"))?;
    match url.scheme() {
        "wss" => {}
        "ws" if allow_insecure => {}
        "ws" => {
            let port = url.port_or_known_default().ok_or_else(|| {
                anyhow::anyhow!("sensor notifier websocket URL must include a port")
            })?;
            let lookup_host = host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .unwrap_or(host);
            let addresses = tokio::net::lookup_host((lookup_host, port))
                .await
                .map_err(|error| {
                    anyhow::anyhow!(
                        "failed to resolve sensor notifier websocket host '{host}': {error}"
                    )
                })?
                .collect::<Vec<_>>();
            if addresses.is_empty() || addresses.iter().any(|address| !address.ip().is_loopback()) {
                anyhow::bail!(
                    "non-loopback sensor notifier websocket URLs require wss:// or sensor.allow_insecure_notifier_ws=true"
                );
            }
        }
        _ => anyhow::bail!("sensor notifier websocket URL must use wss:// or ws://"), // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Validation error names the only accepted websocket schemes; it is not an endpoint.
    }

    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(
            HealthStatus::Degraded("test".to_string()).to_string(),
            "degraded: test"
        );
        assert_eq!(
            HealthStatus::Unhealthy("error".to_string()).to_string(),
            "unhealthy: error"
        );
    }

    #[tokio::test]
    async fn notifier_ws_url_allows_secure_remote_endpoint() {
        assert_eq!(
            validate_notifier_ws_url("wss://notify.example/ws", false)
                .await
                .unwrap(),
            "wss://notify.example/ws"
        );
    }

    #[tokio::test]
    async fn notifier_ws_url_allows_plaintext_only_when_every_address_is_loopback() {
        // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test for the loopback exception.
        assert!(validate_notifier_ws_url("ws://127.0.0.1:8081/ws", false)
            .await
            .is_ok());
        // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test for the IPv6 loopback exception.
        assert!(validate_notifier_ws_url("ws://[::1]:8081/ws", false)
            .await
            .is_ok());
        // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test that all localhost DNS results are loopback.
        assert!(validate_notifier_ws_url("ws://localhost:8081/ws", false)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn notifier_ws_url_requires_opt_in_for_plaintext_remote_endpoint() {
        let url = "ws://notifier:8081/ws"; // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test for rejecting non-loopback plaintext.
        assert!(validate_notifier_ws_url(url, false).await.is_err());
        assert!(validate_notifier_ws_url(url, true).await.is_ok());
    }

    #[tokio::test]
    async fn notifier_ws_url_rejects_credentials_and_malformed_hosts() {
        assert!(
            validate_notifier_ws_url("wss://user:secret@notify.example/ws", false)
                .await
                .is_err()
        );
        assert!(validate_notifier_ws_url("wss://[::1", false).await.is_err());
    }

    #[test]
    fn legacy_notifier_ws_url_takes_precedence_over_structured_config() {
        let config: Config = serde_json::from_value(serde_json::json!({
            "sensor": {"notifier_ws_url": "wss://structured.example/ws"}
        }))
        .unwrap();

        assert_eq!(
            select_notifier_ws_url(&config, Some("wss://legacy.example/ws".to_string())),
            "wss://legacy.example/ws"
        );
    }

    #[test]
    fn missing_notifier_client_url_uses_only_loopback_fallback() {
        let config: Config = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            select_notifier_ws_url(&config, None),
            "ws://127.0.0.1:8081/ws" // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test for the local-only fallback.
        );
    }
}
