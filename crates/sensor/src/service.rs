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

        let notifier_ws_url = resolve_notifier_ws_url(&config);

        let sensor_manager = Arc::new(SensorManager::new(
            db.clone(),
            SensorManagerConfig {
                api_url,
                worker_token_provider: Some(worker_token_provider),
                notifier_ws_url,
                mq_url,
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

fn resolve_notifier_ws_url(config: &Config) -> String {
    if let Ok(url) = std::env::var("ATTUNE_NOTIFIER_WS_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let (host, port) = config
        .notifier
        .as_ref()
        .map(|notifier| (notifier.host.as_str(), notifier.port))
        .unwrap_or(("127.0.0.1", 8081));
    notifier_ws_url_from_host(host, port)
}

fn notifier_ws_url_from_host(host: &str, port: u16) -> String {
    let host = match host {
        "0.0.0.0" | "::" => "127.0.0.1",
        value => value,
    };

    format!("ws://{}:{}/ws", host, port)
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

    #[test]
    fn notifier_ws_url_uses_loopback_for_unspecified_bind_host() {
        assert_eq!(
            notifier_ws_url_from_host("0.0.0.0", 8081),
            "ws://127.0.0.1:8081/ws"
        );
    }

    #[test]
    fn notifier_ws_url_preserves_explicit_host() {
        assert_eq!(
            notifier_ws_url_from_host("notifier", 8081),
            "ws://notifier:8081/ws"
        );
    }
}
