//! Worker Registration Module
//!
//! Handles worker registration, discovery, and status management in the database.
//! Uses unified runtime detection from the common crate.

use attune_common::config::Config;
use attune_common::error::{Error, Result};
use attune_common::models::{Worker, WorkerRole, WorkerStatus, WorkerType};
use attune_common::runtime_detection::{normalize_runtime_name, RuntimeDetector};
use attune_common::scheduling::{
    validate_label_map, validate_taints, WORKER_LABELS_CAPABILITY_KEY, WORKER_TAINTS_CAPABILITY_KEY,
};
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::runtime_detect::DetectedRuntime;
use crate::version_verify::RUNTIME_VERSIONS_CAPABILITY_KEY;

const ATTUNE_AGENT_MODE_ENV: &str = "ATTUNE_AGENT_MODE";
const ATTUNE_AGENT_BINARY_NAME_ENV: &str = "ATTUNE_AGENT_BINARY_NAME";
const ATTUNE_AGENT_BINARY_VERSION_ENV: &str = "ATTUNE_AGENT_BINARY_VERSION";

/// Worker registration manager
pub struct WorkerRegistration {
    pool: PgPool,
    worker_id: Option<i64>,
    worker_name: String,
    worker_type: WorkerType,
    worker_role: WorkerRole,
    runtime_id: Option<i64>,
    host: Option<String>,
    port: Option<i32>,
    capabilities: HashMap<String, serde_json::Value>,
}

impl WorkerRegistration {
    fn env_truthy(name: &str) -> bool {
        std::env::var(name)
            .ok()
            .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
            .unwrap_or(false)
    }

    fn inject_agent_capabilities(capabilities: &mut HashMap<String, serde_json::Value>) {
        if Self::env_truthy(ATTUNE_AGENT_MODE_ENV) {
            capabilities.insert("agent_mode".to_string(), json!(true));
        }

        if let Ok(binary_name) = std::env::var(ATTUNE_AGENT_BINARY_NAME_ENV) {
            let binary_name = binary_name.trim();
            if !binary_name.is_empty() {
                capabilities.insert("agent_binary_name".to_string(), json!(binary_name));
            }
        }

        if let Ok(binary_version) = std::env::var(ATTUNE_AGENT_BINARY_VERSION_ENV) {
            let binary_version = binary_version.trim();
            if !binary_version.is_empty() {
                capabilities.insert("agent_binary_version".to_string(), json!(binary_version));
            }
        }
    }

    fn legacy_worker_name() -> Option<String> {
        std::env::var("ATTUNE_WORKER_NAME")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn legacy_worker_type() -> Option<WorkerType> {
        let value = std::env::var("ATTUNE_WORKER_TYPE").ok()?;
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Some(WorkerType::Local),
            "remote" => Some(WorkerType::Remote),
            "container" => Some(WorkerType::Container),
            other => {
                warn!("Ignoring unrecognized ATTUNE_WORKER_TYPE value: {}", other);
                None
            }
        }
    }

    /// Create a new worker registration manager
    pub fn new(pool: PgPool, config: &Config) -> Self {
        let worker_name = config
            .worker
            .as_ref()
            .and_then(|w| w.name.clone())
            .or_else(Self::legacy_worker_name)
            .unwrap_or_else(|| {
                format!(
                    "worker-{}",
                    hostname::get()
                        .unwrap_or_else(|_| "unknown".into())
                        .to_string_lossy()
                )
            });

        let worker_type = config
            .worker
            .as_ref()
            .and_then(|w| w.worker_type)
            .or_else(Self::legacy_worker_type)
            .unwrap_or(WorkerType::Local);

        let worker_role = WorkerRole::Action;

        let runtime_id = config.worker.as_ref().and_then(|w| w.runtime_id);

        let host = config
            .worker
            .as_ref()
            .and_then(|w| w.host.clone())
            .or_else(|| {
                hostname::get()
                    .ok()
                    .map(|h| h.to_string_lossy().to_string())
            });

        let port = config.worker.as_ref().and_then(|w| w.port);

        // Initial capabilities (will be populated asynchronously)
        let mut capabilities = HashMap::new();

        // Set max_concurrent_executions from config
        let max_concurrent = config
            .worker
            .as_ref()
            .map(|w| w.max_concurrent_tasks)
            .unwrap_or(10);
        capabilities.insert(
            "max_concurrent_executions".to_string(),
            json!(max_concurrent),
        );

        // Add worker version metadata
        capabilities.insert(
            "worker_version".to_string(),
            json!(env!("CARGO_PKG_VERSION")),
        );

        Self::inject_agent_capabilities(&mut capabilities);

        if let Some(worker_config) = config.worker.as_ref() {
            if let Err(err) = validate_label_map("worker.labels", &worker_config.labels) {
                warn!(
                    "Ignoring invalid worker labels for '{}': {}",
                    worker_name, err
                );
            } else if !worker_config.labels.is_empty() {
                capabilities.insert(
                    WORKER_LABELS_CAPABILITY_KEY.to_string(),
                    json!(worker_config.labels),
                );
            }

            if let Err(err) = validate_taints(&worker_config.taints) {
                warn!(
                    "Ignoring invalid worker taints for '{}': {}",
                    worker_name, err
                );
            } else if !worker_config.taints.is_empty() {
                capabilities.insert(
                    WORKER_TAINTS_CAPABILITY_KEY.to_string(),
                    json!(worker_config.taints),
                );
            }
        }

        // Placeholder for runtimes (will be detected asynchronously)
        capabilities.insert("runtimes".to_string(), json!(Vec::<String>::new()));

        Self {
            pool,
            worker_id: None,
            worker_name,
            worker_type,
            worker_role,
            runtime_id,
            host,
            port,
            capabilities,
        }
    }

    /// Store detected runtime interpreter metadata in capabilities.
    ///
    /// This is used by the agent (`attune-agent`) to record the full details of
    /// auto-detected interpreters — binary paths and versions — alongside the
    /// simple `runtimes` string list used for backward compatibility.
    ///
    /// The data is stored under the `detected_interpreters` capability key as a
    /// JSON array of objects:
    /// ```json
    /// [
    ///   {"name": "python", "path": "/usr/bin/python3", "version": "3.12.1"},
    ///   {"name": "shell", "path": "/bin/bash", "version": "5.2.15"}
    /// ]
    /// ```
    pub fn set_detected_runtimes(&mut self, runtimes: Vec<DetectedRuntime>) {
        let mut runtime_versions: HashMap<String, Vec<String>> = HashMap::new();
        let interpreters: Vec<serde_json::Value> = runtimes
            .iter()
            .map(|rt| {
                if let Some(version) = rt
                    .version
                    .as_ref()
                    .filter(|version| !version.trim().is_empty())
                {
                    runtime_versions
                        .entry(normalize_runtime_name(&rt.name))
                        .or_default()
                        .push(version.clone());
                }

                json!({
                    "name": rt.name,
                    "path": rt.path,
                    "version": rt.version,
                })
            })
            .collect();

        for versions in runtime_versions.values_mut() {
            versions.sort();
            versions.dedup();
        }

        self.capabilities
            .insert("detected_interpreters".to_string(), json!(interpreters));
        self.capabilities.insert(
            RUNTIME_VERSIONS_CAPABILITY_KEY.to_string(),
            json!(runtime_versions),
        );

        info!(
            "Stored {} detected interpreter(s) in capabilities",
            runtimes.len()
        );
    }

    /// Mark this worker as running in agent mode.
    ///
    /// Agent-mode workers auto-detect their runtimes at startup (as opposed to
    /// being configured via `ATTUNE_WORKER_RUNTIMES` or config files). Setting
    /// this flag allows the system to distinguish agents from standard workers.
    pub fn set_agent_mode(&mut self, is_agent: bool) {
        self.capabilities
            .insert("agent_mode".to_string(), json!(is_agent));
    }

    /// Detect available runtimes using the unified runtime detector
    pub async fn detect_capabilities(&mut self, config: &Config) -> Result<()> {
        info!("Detecting worker capabilities...");

        let detector = RuntimeDetector::new(self.pool.clone());

        // Get config capabilities if available
        let config_capabilities = config.worker.as_ref().and_then(|w| w.capabilities.as_ref());

        // Detect capabilities with three-tier priority:
        // 1. ATTUNE_WORKER_RUNTIMES env var
        // 2. Config file
        // 3. Database-driven detection
        let detected_capabilities = detector
            .detect_capabilities(config, "ATTUNE_WORKER_RUNTIMES", config_capabilities)
            .await?;

        // Merge detected capabilities with existing ones
        for (key, value) in detected_capabilities {
            self.capabilities.insert(key, value);
        }

        info!("Worker capabilities detected: {:?}", self.capabilities);

        Ok(())
    }

    /// Register the worker in the database
    pub async fn register(&mut self) -> Result<i64> {
        info!("Registering worker: {}", self.worker_name);

        // Check if worker with this name already exists
        let existing = sqlx::query_as::<_, Worker>(
            "SELECT * FROM worker WHERE name = $1 ORDER BY created DESC LIMIT 1",
        )
        .bind(&self.worker_name)
        .fetch_optional(&self.pool)
        .await?;

        let worker_id = if let Some(existing_worker) = existing {
            info!(
                "Worker '{}' already exists (ID: {}), updating status",
                self.worker_name, existing_worker.id
            );

            // Update existing worker to active status with new heartbeat
            sqlx::query(
                r#"
                UPDATE worker
                SET status = $1,
                    last_heartbeat = $2,
                    host = $3,
                    port = $4,
                    capabilities = $5,
                    updated = $2
                WHERE id = $6
                "#,
            )
            .bind(WorkerStatus::Active)
            .bind(Utc::now())
            .bind(&self.host)
            .bind(self.port)
            .bind(serde_json::to_value(&self.capabilities)?)
            .bind(existing_worker.id)
            .execute(&self.pool)
            .await?;

            existing_worker.id
        } else {
            info!("Creating new worker registration: {}", self.worker_name);

            // Insert new worker
            let worker = sqlx::query_as::<_, Worker>(
                r#"
                INSERT INTO worker (name, worker_type, worker_role, runtime, host, port, status, capabilities, last_heartbeat)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                RETURNING *
                "#,
            )
            .bind(&self.worker_name)
            .bind(self.worker_type)
            .bind(self.worker_role)
            .bind(self.runtime_id)
            .bind(&self.host)
            .bind(self.port)
            .bind(WorkerStatus::Active)
            .bind(serde_json::to_value(&self.capabilities)?)
            .bind(Utc::now())
            .fetch_one(&self.pool)
            .await?;

            worker.id
        };

        self.worker_id = Some(worker_id);
        info!("Worker registered successfully with ID: {}", worker_id);

        Ok(worker_id)
    }

    /// Deregister the worker (mark as inactive)
    pub async fn deregister(&self) -> Result<()> {
        if let Some(worker_id) = self.worker_id {
            info!("Deregistering worker ID: {}", worker_id);

            sqlx::query(
                r#"
                UPDATE worker
                SET status = $1,
                    updated = $2
                WHERE id = $3
                "#,
            )
            .bind(WorkerStatus::Inactive)
            .bind(Utc::now())
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

            info!("Worker deregistered successfully");
        } else {
            warn!("Cannot deregister: worker not registered");
        }

        Ok(())
    }

    /// Update worker heartbeat
    pub async fn update_heartbeat(&self) -> Result<()> {
        if let Some(worker_id) = self.worker_id {
            sqlx::query(
                r#"
                UPDATE worker
                SET last_heartbeat = $1,
                    status = $2,
                    updated = $1
                WHERE id = $3
                "#,
            )
            .bind(Utc::now())
            .bind(WorkerStatus::Active)
            .bind(worker_id)
            .execute(&self.pool)
            .await?;
        } else {
            return Err(Error::invalid_state("Worker not registered"));
        }

        Ok(())
    }

    /// Get the registered worker ID
    pub fn worker_id(&self) -> Option<i64> {
        self.worker_id
    }

    /// Get the worker name
    pub fn worker_name(&self) -> &str {
        &self.worker_name
    }

    /// Add a capability to the worker
    pub fn add_capability(&mut self, key: String, value: serde_json::Value) {
        self.capabilities.insert(key, value);
    }

    /// Update worker capabilities in the database
    pub async fn update_capabilities(&self) -> Result<()> {
        if let Some(worker_id) = self.worker_id {
            sqlx::query(
                r#"
                UPDATE worker
                SET capabilities = $1,
                    updated = $2
                WHERE id = $3
                "#,
            )
            .bind(serde_json::to_value(&self.capabilities)?)
            .bind(Utc::now())
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

            info!("Worker capabilities updated");
        }

        Ok(())
    }
}

impl Drop for WorkerRegistration {
    fn drop(&mut self) {
        // Note: We can't make this async, so we just log
        // The main service should call deregister() explicitly during shutdown
        if self.worker_id.is_some() {
            info!("WorkerRegistration dropped - worker should be deregistered");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::test_database::TestDatabase;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static AGENT_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard(Vec<(&'static str, Option<OsString>)>);

    impl EnvGuard {
        fn preserve(names: &[&'static str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.0 {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    fn test_config() -> Config {
        Config::load_from_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config.test.yaml"
        ))
        .unwrap()
    }

    async fn isolated_test_context() -> (Config, PgPool, TestDatabase) {
        let mut config = test_config();
        if let Some(worker) = config.worker.as_mut() {
            worker.name = Some(format!("worker-test-{}", uuid::Uuid::new_v4().simple()));
        }
        let database = TestDatabase::create(&config.database)
            .await
            .expect("Failed to create isolated worker test database")
            .with_cleanup_on_drop();
        (config, database.pool().clone(), database)
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_worker_registration() {
        let (config, pool, _database) = isolated_test_context().await;
        let mut registration = WorkerRegistration::new(pool, &config);

        // Detect capabilities
        registration.detect_capabilities(&config).await.unwrap();

        // Register worker
        let worker_id = registration.register().await.unwrap();
        assert!(worker_id > 0);
        assert_eq!(registration.worker_id(), Some(worker_id));

        // Update heartbeat
        registration.update_heartbeat().await.unwrap();

        // Deregister worker
        registration.deregister().await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_worker_capabilities() {
        let (config, pool, _database) = isolated_test_context().await;
        let mut registration = WorkerRegistration::new(pool, &config);

        registration.detect_capabilities(&config).await.unwrap();
        registration.register().await.unwrap();

        // Add capability
        registration.add_capability("test_capability".to_string(), json!(true));
        registration.update_capabilities().await.unwrap();

        registration.deregister().await.unwrap();
    }

    #[test]
    fn test_detected_runtimes_json_structure() {
        // Test the JSON structure that set_detected_runtimes builds
        let runtimes = [
            DetectedRuntime {
                name: "python".to_string(),
                path: "/usr/bin/python3".to_string(),
                version: Some("3.12.1".to_string()),
            },
            DetectedRuntime {
                name: "shell".to_string(),
                path: "/bin/bash".to_string(),
                version: None,
            },
        ];

        let interpreters: Vec<serde_json::Value> = runtimes
            .iter()
            .map(|rt| {
                json!({
                    "name": rt.name,
                    "path": rt.path,
                    "version": rt.version,
                })
            })
            .collect();

        let json_value = json!(interpreters);

        // Verify structure
        let arr = json_value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "python");
        assert_eq!(arr[0]["path"], "/usr/bin/python3");
        assert_eq!(arr[0]["version"], "3.12.1");
        assert_eq!(arr[1]["name"], "shell");
        assert_eq!(arr[1]["path"], "/bin/bash");
        assert!(arr[1]["version"].is_null());
    }

    #[test]
    fn test_detected_runtimes_empty() {
        let runtimes: Vec<DetectedRuntime> = vec![];
        let interpreters: Vec<serde_json::Value> = runtimes
            .iter()
            .map(|rt| {
                json!({
                    "name": rt.name,
                    "path": rt.path,
                    "version": rt.version,
                })
            })
            .collect();

        let json_value = json!(interpreters);
        assert_eq!(json_value.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_set_detected_runtimes_populates_runtime_versions_capability() {
        let pool = PgPool::connect_lazy("postgres://localhost/test").unwrap();
        let config = Config {
            service_name: "attune".to_string(),
            environment: "test".to_string(),
            default_execution_timeout_seconds: 600,
            database: attune_common::config::DatabaseConfig::default(),
            message_queue: None,
            server: attune_common::config::ServerConfig::default(),
            log: attune_common::config::LogConfig::default(),
            security: attune_common::config::SecurityConfig::default(),
            worker: None,
            sensor: None,
            packs_base_dir: "/tmp/packs".to_string(),
            runtime_envs_dir: "/tmp/runtime_envs".to_string(),
            artifacts_dir: "/tmp/artifacts".to_string(),
            notifier: None,
            pack_registry: attune_common::config::PackRegistryConfig::default(),
            executor: None,
            agent: None,
            pack_upload: attune_common::config::PackUploadConfig::default(),
            artifacts: attune_common::config::ArtifactsConfig::default(),
            retention: attune_common::config::RetentionConfig::default(),
            maintenance: attune_common::config::SupervisorMaintenanceConfig::default(),
            cache_retention: attune_common::config::CacheRetentionConfig::default(),
            cache_admission: attune_common::config::CacheAdmissionConfig::default(),
        };
        let mut registration = WorkerRegistration::new(pool, &config);

        registration.set_detected_runtimes(vec![
            DetectedRuntime {
                name: "python".to_string(),
                path: "/usr/bin/python3".to_string(),
                version: Some("3.12.13".to_string()),
            },
            DetectedRuntime {
                name: "nodejs".to_string(),
                path: "/usr/bin/node".to_string(),
                version: Some("20.19.0".to_string()),
            },
        ]);

        assert_eq!(
            registration
                .capabilities
                .get(RUNTIME_VERSIONS_CAPABILITY_KEY),
            Some(&json!({
                "node": ["20.19.0"],
                "python": ["3.12.13"]
            }))
        );
    }

    #[test]
    fn test_agent_mode_capability_value() {
        // Verify the JSON value for agent_mode capability
        let value = json!(true);
        assert_eq!(value, true);

        let value = json!(false);
        assert_eq!(value, false);
    }

    #[test]
    fn test_inject_agent_capabilities_from_env() {
        let _lock = AGENT_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::preserve(&[
            ATTUNE_AGENT_MODE_ENV,
            ATTUNE_AGENT_BINARY_NAME_ENV,
            ATTUNE_AGENT_BINARY_VERSION_ENV,
        ]);
        std::env::set_var(ATTUNE_AGENT_MODE_ENV, "TRUE");
        std::env::set_var(ATTUNE_AGENT_BINARY_NAME_ENV, "attune-agent");
        std::env::set_var(ATTUNE_AGENT_BINARY_VERSION_ENV, "1.2.3");

        let mut capabilities = HashMap::new();
        WorkerRegistration::inject_agent_capabilities(&mut capabilities);

        assert_eq!(capabilities.get("agent_mode"), Some(&json!(true)));
        assert_eq!(
            capabilities.get("agent_binary_name"),
            Some(&json!("attune-agent"))
        );
        assert_eq!(
            capabilities.get("agent_binary_version"),
            Some(&json!("1.2.3"))
        );
    }
}
