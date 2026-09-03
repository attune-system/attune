//! Sensor Worker Registration Module
//!
//! Handles sensor worker registration, discovery, and status management in the database.
//! Similar to action worker registration but tailored for sensor service instances.
//!
//! Runtime detection uses the unified RuntimeDetector from common crate.

use crate::sensor_manager::cancellable_command_output;
use attune_common::agent_runtime_detection::{detect_runtimes, DetectedRuntime};
use attune_common::config::Config;
use attune_common::error::Result;
use attune_common::models::{RuntimeVersion, Worker, WorkerRole, WorkerStatus, WorkerType};
use attune_common::repositories::{List, RuntimeRepository, RuntimeVersionRepository};
use attune_common::runtime_detection::{normalize_runtime_name, RuntimeDetector};
use attune_common::scheduling::{
    validate_label_map, validate_taints, WORKER_LABELS_CAPABILITY_KEY, WORKER_TAINTS_CAPABILITY_KEY,
};
use chrono::Utc;
use serde_json::json;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info};

/// Capability key under which detected runtime versions are stored.
///
/// Mirrors `attune_worker::version_verify::RUNTIME_VERSIONS_CAPABILITY_KEY` so
/// the API surfaces version info uniformly for both action and sensor workers.
const RUNTIME_VERSIONS_CAPABILITY_KEY: &str = "runtime_versions";

const ATTUNE_SENSOR_AGENT_MODE_ENV: &str = "ATTUNE_SENSOR_AGENT_MODE";
const ATTUNE_SENSOR_AGENT_BINARY_NAME_ENV: &str = "ATTUNE_SENSOR_AGENT_BINARY_NAME";
const ATTUNE_SENSOR_AGENT_BINARY_VERSION_ENV: &str = "ATTUNE_SENSOR_AGENT_BINARY_VERSION";

/// Sensor worker registration manager
pub struct SensorWorkerRegistration {
    pool: PgPool,
    worker_id: Option<i64>,
    worker_name: String,
    host: Option<String>,
    capabilities: HashMap<String, serde_json::Value>,
}

impl SensorWorkerRegistration {
    fn env_truthy(name: &str) -> bool {
        std::env::var(name)
            .ok()
            .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
            .unwrap_or(false)
    }

    fn inject_agent_capabilities(capabilities: &mut HashMap<String, serde_json::Value>) {
        if Self::env_truthy(ATTUNE_SENSOR_AGENT_MODE_ENV) {
            capabilities.insert("agent_mode".to_string(), json!(true));
        }

        if let Ok(binary_name) = std::env::var(ATTUNE_SENSOR_AGENT_BINARY_NAME_ENV) {
            let binary_name = binary_name.trim();
            if !binary_name.is_empty() {
                capabilities.insert("agent_binary_name".to_string(), json!(binary_name));
            }
        }

        if let Ok(binary_version) = std::env::var(ATTUNE_SENSOR_AGENT_BINARY_VERSION_ENV) {
            let binary_version = binary_version.trim();
            if !binary_version.is_empty() {
                capabilities.insert("agent_binary_version".to_string(), json!(binary_version));
            }
        }
    }

    /// Store detected runtime interpreter metadata (paths + versions) in
    /// capabilities. Mirrors `attune_worker::WorkerRegistration::set_detected_runtimes`.
    ///
    /// Writes three capability keys:
    /// - `detected_interpreters`: array of {name, path, version} objects
    /// - `runtime_versions`: map of normalized runtime name -> sorted versions
    /// - `runtimes`: list of normalized runtime names (overrides DB-driven names)
    pub fn set_detected_runtimes(&mut self, runtimes: Vec<DetectedRuntime>) {
        let configured_runtime_names = self
            .capabilities
            .get("runtimes")
            .and_then(|value| value.as_array())
            .and_then(|names| {
                let names = names
                    .iter()
                    .filter_map(|name| name.as_str())
                    .map(normalize_runtime_name)
                    .collect::<std::collections::HashSet<_>>();
                (!names.is_empty()).then_some(names)
            })
            .or_else(|| {
                std::env::var("ATTUNE_SENSOR_RUNTIMES")
                    .ok()
                    .and_then(|value| {
                        let names = value
                            .split(',')
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(normalize_runtime_name)
                            .collect::<std::collections::HashSet<_>>();
                        (!names.is_empty()).then_some(names)
                    })
            });
        let runtimes = runtimes
            .into_iter()
            .filter(|runtime| {
                configured_runtime_names.as_ref().is_none_or(|configured| {
                    configured.contains(&normalize_runtime_name(&runtime.name))
                })
            })
            .collect::<Vec<_>>();
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

        let mut runtime_names = match configured_runtime_names {
            Some(names) => names.into_iter().collect::<Vec<_>>(),
            None => {
                let mut names = runtime_versions.keys().cloned().collect::<Vec<_>>();
                for rt in &runtimes {
                    let normalized = normalize_runtime_name(&rt.name);
                    if !names.contains(&normalized) {
                        names.push(normalized);
                    }
                }
                names
            }
        };
        runtime_names.sort();

        self.capabilities
            .insert("detected_interpreters".to_string(), json!(interpreters));
        self.capabilities.insert(
            RUNTIME_VERSIONS_CAPABILITY_KEY.to_string(),
            json!(runtime_versions),
        );
        self.capabilities
            .insert("runtimes".to_string(), json!(runtime_names));

        info!(
            "Stored {} detected interpreter(s) in sensor capabilities",
            runtimes.len()
        );
    }

    /// Mark this sensor worker as running in agent mode.
    pub fn set_agent_mode(&mut self, enabled: bool) {
        self.capabilities
            .insert("agent_mode".to_string(), json!(enabled));
    }

    /// Create a new sensor worker registration manager
    pub fn new(pool: PgPool, config: &Config) -> Self {
        let worker_name = config
            .sensor
            .as_ref()
            .and_then(|s| s.worker_name.clone())
            .unwrap_or_else(|| {
                format!(
                    "sensor-{}",
                    hostname::get()
                        .unwrap_or_else(|_| "unknown".into())
                        .to_string_lossy()
                )
            });

        let host = config
            .sensor
            .as_ref()
            .and_then(|s| s.host.clone())
            .or_else(|| {
                hostname::get()
                    .ok()
                    .map(|h| h.to_string_lossy().to_string())
            });

        // Initial capabilities (will be populated asynchronously)
        let mut capabilities = HashMap::new();

        // Set max_concurrent_sensors from config
        let max_concurrent = config
            .sensor
            .as_ref()
            .and_then(|s| s.max_concurrent_sensors)
            .unwrap_or(10);
        capabilities.insert("max_concurrent_sensors".to_string(), json!(max_concurrent));
        capabilities.insert("sensor_processes_monitored".to_string(), json!(0));
        capabilities.insert("sensor_processes_running".to_string(), json!(0));
        capabilities.insert("active_rules".to_string(), json!(0));

        // Add sensor worker version metadata
        capabilities.insert(
            "sensor_version".to_string(),
            json!(env!("CARGO_PKG_VERSION")),
        );

        Self::inject_agent_capabilities(&mut capabilities);

        if let Some(sensor_config) = config.sensor.as_ref() {
            if let Err(err) = validate_label_map("sensor.labels", &sensor_config.labels) {
                tracing::warn!(
                    "Ignoring invalid sensor worker labels for '{}': {}",
                    worker_name,
                    err
                );
            } else if !sensor_config.labels.is_empty() {
                capabilities.insert(
                    WORKER_LABELS_CAPABILITY_KEY.to_string(),
                    json!(sensor_config.labels),
                );
            }

            if let Err(err) = validate_taints(&sensor_config.taints) {
                tracing::warn!(
                    "Ignoring invalid sensor worker taints for '{}': {}",
                    worker_name,
                    err
                );
            } else if !sensor_config.taints.is_empty() {
                capabilities.insert(
                    WORKER_TAINTS_CAPABILITY_KEY.to_string(),
                    json!(sensor_config.taints),
                );
            }
        }

        // Placeholder for runtimes (will be detected asynchronously)
        capabilities.insert("runtimes".to_string(), json!(Vec::<String>::new()));

        Self {
            pool,
            worker_id: None,
            worker_name,
            host,
            capabilities,
        }
    }

    /// Register the sensor worker in the database
    pub async fn register(&mut self, config: &Config) -> Result<i64> {
        // Detect runtimes from database if not already configured
        self.detect_capabilities_async(config).await?;

        info!("Registering sensor worker: {}", self.worker_name);

        // Check if sensor worker with this name already exists
        let existing = sqlx::query_as::<_, Worker>(
            "SELECT * FROM worker WHERE name = $1 AND worker_role = 'sensor' ORDER BY created DESC LIMIT 1",
        )
        .bind(&self.worker_name)
        .fetch_optional(&self.pool)
        .await?;

        let worker_id = if let Some(existing_worker) = existing {
            info!(
                "Sensor worker '{}' already exists (ID: {}), updating status",
                self.worker_name, existing_worker.id
            );

            // Update existing sensor worker to active status with new heartbeat
            sqlx::query(
                r#"
                UPDATE worker
                SET status = $1,
                    capabilities = $2,
                    last_heartbeat = $3,
                    updated = $4,
                    host = $5
                WHERE id = $6
                "#,
            )
            .bind(WorkerStatus::Active)
            .bind(serde_json::to_value(&self.capabilities)?)
            .bind(Utc::now())
            .bind(Utc::now())
            .bind(&self.host)
            .bind(existing_worker.id)
            .execute(&self.pool)
            .await?;

            existing_worker.id
        } else {
            // Insert new sensor worker
            let row = sqlx::query(
                r#"
                INSERT INTO worker (name, worker_type, worker_role, host, status, capabilities, last_heartbeat)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING id
                "#,
            )
            .bind(&self.worker_name)
            .bind(WorkerType::Local) // Sensor workers are always local
            .bind(WorkerRole::Sensor)
            .bind(&self.host)
            .bind(WorkerStatus::Active)
            .bind(serde_json::to_value(&self.capabilities)?)
            .bind(Utc::now())
            .fetch_one(&self.pool)
            .await?;

            let worker_id: i64 = row.get("id");
            info!("Sensor worker registered with ID: {}", worker_id);
            worker_id
        };

        self.worker_id = Some(worker_id);
        Ok(worker_id)
    }

    /// Send heartbeat to update last_heartbeat timestamp
    pub async fn heartbeat(&self) -> Result<()> {
        if let Some(worker_id) = self.worker_id {
            sqlx::query(
                r#"
                UPDATE worker
                SET last_heartbeat = $1,
                    status = $2,
                    updated = $3
                WHERE id = $4
                "#,
            )
            .bind(Utc::now())
            .bind(WorkerStatus::Active)
            .bind(Utc::now())
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

            debug!("Sensor worker heartbeat sent");
        }

        Ok(())
    }

    /// Mark sensor worker as inactive
    pub async fn deregister(&self) -> Result<()> {
        if let Some(worker_id) = self.worker_id {
            info!("Deregistering sensor worker: {}", self.worker_name);

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

            info!("Sensor worker deregistered");
        }

        Ok(())
    }

    /// Get the registered sensor worker ID
    pub fn worker_id(&self) -> Option<i64> {
        self.worker_id
    }

    /// Get the sensor worker name
    pub fn worker_name(&self) -> &str {
        &self.worker_name
    }

    /// Add or replace a capability on the sensor worker.
    ///
    /// Returns true when the stored value changed.
    pub fn add_capability(&mut self, key: String, value: serde_json::Value) -> bool {
        let changed = self.capabilities.get(&key) != Some(&value);
        self.capabilities.insert(key, value);
        changed
    }

    /// Update sensor worker capabilities in the database
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

            info!("Sensor worker capabilities updated");
        }

        Ok(())
    }

    /// Detect sensor worker capabilities based on database-driven runtime verification
    ///
    /// This is a synchronous wrapper that should be called after pool is available.
    /// The actual detection happens in `detect_capabilities_async`.
    /// Detect available runtimes using the unified runtime detector
    pub async fn detect_capabilities_async(&mut self, config: &Config) -> Result<()> {
        // If the agent has already populated runtime_versions via
        // `set_detected_runtimes`, skip DB-driven detection so we don't
        // clobber the agent-provided interpreter metadata.
        if self
            .capabilities
            .contains_key(RUNTIME_VERSIONS_CAPABILITY_KEY)
        {
            info!(
                "Sensor capabilities already populated by agent detection; \
                 skipping DB-driven detection"
            );
            return Ok(());
        }

        info!("Detecting sensor worker capabilities...");

        let detector = RuntimeDetector::new(self.pool.clone());

        // Get config capabilities if available
        let config_capabilities = config.sensor.as_ref().and_then(|s| s.capabilities.as_ref());

        // Detect capabilities with three-tier priority:
        // 1. ATTUNE_SENSOR_RUNTIMES env var
        // 2. Config file
        // 3. Database-driven detection
        let detected_capabilities = detector
            .detect_capabilities(config, "ATTUNE_SENSOR_RUNTIMES", config_capabilities)
            .await?;

        // Merge detected capabilities with existing ones
        for (key, value) in detected_capabilities {
            self.capabilities.insert(key, value);
        }
        self.set_detected_runtimes(detect_runtimes());
        self.add_registered_runtime_versions().await?;

        info!(
            "Sensor worker capabilities detected: {:?}",
            self.capabilities
        );

        Ok(())
    }

    async fn add_registered_runtime_versions(&mut self) -> Result<()> {
        let configured = self
            .capabilities
            .get("runtimes")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str())
            .map(normalize_runtime_name)
            .collect::<std::collections::HashSet<_>>();
        let mut verified = self
            .capabilities
            .get(RUNTIME_VERSIONS_CAPABILITY_KEY)
            .and_then(|value| {
                serde_json::from_value::<HashMap<String, Vec<String>>>(value.clone()).ok()
            })
            .unwrap_or_default();

        let runtimes = RuntimeRepository::list(&self.pool)
            .await?
            .into_iter()
            .map(|runtime| {
                let runtime_name = normalize_runtime_name(&runtime.name);
                let names = runtime
                    .aliases
                    .iter()
                    .map(|alias| normalize_runtime_name(alias))
                    .chain(std::iter::once(runtime_name.clone()))
                    .collect::<std::collections::HashSet<_>>();
                (runtime.id, (runtime_name, names))
            })
            .collect::<HashMap<_, _>>();
        for version in RuntimeVersionRepository::list(&self.pool).await? {
            let Some((runtime_name, runtime_names)) = runtimes.get(&version.runtime) else {
                continue;
            };
            if !configured.is_disjoint(runtime_names)
                && verify_registered_runtime_version(&version).await
            {
                verified
                    .entry(runtime_name.clone())
                    .or_default()
                    .push(version.version);
            }
        }
        for versions in verified.values_mut() {
            versions.sort();
            versions.dedup();
        }
        self.capabilities
            .insert(RUNTIME_VERSIONS_CAPABILITY_KEY.to_string(), json!(verified));
        Ok(())
    }
}

async fn verify_registered_runtime_version(version: &RuntimeVersion) -> bool {
    let commands = version
        .distributions
        .get("verification")
        .and_then(|value| value.get("commands"))
        .and_then(|value| value.as_array());
    if let Some(commands) = commands {
        let mut commands = commands.iter().collect::<Vec<_>>();
        commands.sort_by_key(|command| {
            command
                .get("priority")
                .and_then(|value| value.as_i64())
                .unwrap_or(100)
        });
        for command in commands {
            let Some(binary) = command.get("binary").and_then(|value| value.as_str()) else {
                continue;
            };
            let args = command
                .get("args")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>();
            let mut process = Command::new(binary);
            process.args(args);
            let Ok(Ok(output)) = tokio::time::timeout(
                Duration::from_secs(10),
                cancellable_command_output(&mut process),
            )
            .await
            else {
                continue;
            };
            let expected_exit = command
                .get("exit_code")
                .and_then(|value| value.as_i64())
                .unwrap_or(0) as i32;
            if output.status.code().unwrap_or(-1) != expected_exit {
                continue;
            }
            if let Some(pattern) = command.get("pattern").and_then(|value| value.as_str()) {
                let Ok(pattern) = regex::Regex::new(pattern) else {
                    continue;
                };
                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                if !pattern.is_match(&combined) {
                    continue;
                }
            }
            return true;
        }
        return false;
    }

    let binary = version.parsed_execution_config().interpreter.binary;
    if binary.is_empty() {
        return false;
    }
    let mut command = Command::new(binary);
    command.arg("--version");
    let Ok(Ok(output)) = tokio::time::timeout(
        Duration::from_secs(10),
        cancellable_command_output(&mut command),
    )
    .await
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    regex::Regex::new(r"(?:v|go)?(\d+(?:\.\d+){0,2})")
        .ok()
        .and_then(|pattern| pattern.captures(&combined))
        .and_then(|captures| captures.get(1))
        .is_some_and(|observed| {
            attune_common::version_matching::runtime_version_matches_worker(
                version,
                observed.as_str(),
            )
        })
}

impl Drop for SensorWorkerRegistration {
    fn drop(&mut self) {
        // Note: We can't make this async, so we just log
        // The main service should call deregister() explicitly during shutdown
        if self.worker_id.is_some() {
            info!("SensorWorkerRegistration dropped - sensor worker should be deregistered");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;
    use std::ffi::OsString;
    use std::sync::LazyLock;
    use tokio::sync::Mutex;

    static AGENT_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

    #[tokio::test]
    async fn detected_versions_preserve_configured_runtime_names() {
        let config = test_config();
        let pool = PgPoolOptions::new()
            .connect_lazy("postgresql://localhost/attune")
            .expect("lazy pool");
        let mut registration = SensorWorkerRegistration::new(pool, &config);
        registration.add_capability("runtimes".to_string(), json!(["native", "python"]));

        registration.set_detected_runtimes(vec![
            DetectedRuntime {
                name: "python".to_string(),
                path: "/usr/bin/python3".to_string(),
                version: Some("3.12.7".to_string()),
            },
            DetectedRuntime {
                name: "node".to_string(),
                path: "/usr/bin/node".to_string(),
                version: Some("20.11.1".to_string()),
            },
        ]);

        assert_eq!(
            registration.capabilities.get("runtimes"),
            Some(&json!(["native", "python"]))
        );
        assert_eq!(
            registration.capabilities.get("runtime_versions"),
            Some(&json!({"python": ["3.12.7"]}))
        );
    }

    #[tokio::test]
    async fn empty_runtime_placeholder_does_not_discard_agent_detection() {
        let _lock = AGENT_ENV_LOCK.lock().await;
        let _env = EnvGuard::preserve(&["ATTUNE_SENSOR_RUNTIMES"]);
        std::env::remove_var("ATTUNE_SENSOR_RUNTIMES");
        let config = test_config();
        let pool = PgPoolOptions::new()
            .connect_lazy("postgresql://localhost/attune")
            .expect("lazy pool");
        let mut registration = SensorWorkerRegistration::new(pool, &config);

        registration.set_detected_runtimes(vec![DetectedRuntime {
            name: "python".to_string(),
            path: "/usr/bin/python3".to_string(),
            version: Some("3.12.7".to_string()),
        }]);

        assert_eq!(
            registration.capabilities.get("runtimes"),
            Some(&json!(["python"]))
        );
    }

    #[tokio::test]
    async fn agent_detection_preserves_override_runtimes_without_interpreters() {
        let _lock = AGENT_ENV_LOCK.lock().await;
        let _env = EnvGuard::preserve(&["ATTUNE_SENSOR_RUNTIMES"]);
        std::env::set_var("ATTUNE_SENSOR_RUNTIMES", "shell,python,native");
        let config = test_config();
        let pool = PgPoolOptions::new()
            .connect_lazy("postgresql://localhost/attune")
            .expect("lazy pool");
        let mut registration = SensorWorkerRegistration::new(pool, &config);

        registration.set_detected_runtimes(vec![DetectedRuntime {
            name: "python".to_string(),
            path: "/usr/bin/python3".to_string(),
            version: Some("3.12.7".to_string()),
        }]);

        assert_eq!(
            registration.capabilities.get("runtimes"),
            Some(&json!(["native", "python", "shell"]))
        );
        assert_eq!(
            registration
                .capabilities
                .get(RUNTIME_VERSIONS_CAPABILITY_KEY),
            Some(&json!({"python": ["3.12.7"]}))
        );
    }

    async fn isolated_test_context() -> (Config, PgPool, attune_common::test_database::TestDatabase)
    {
        let mut config = test_config();
        config
            .sensor
            .as_mut()
            .expect("test sensor config")
            .worker_name = Some(format!("sensor-test-{}", uuid::Uuid::new_v4().simple()));
        let database = attune_common::test_database::TestDatabase::create(&config.database)
            .await
            .expect("Failed to create isolated sensor test database")
            .with_cleanup_on_drop();
        (config, database.pool().clone(), database)
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_database_driven_detection() {
        let _lock = AGENT_ENV_LOCK.lock().await;
        let (config, pool, _database) = isolated_test_context().await;
        let mut registration = SensorWorkerRegistration::new(pool, &config);

        // Detect runtimes from database
        registration
            .detect_capabilities_async(&config)
            .await
            .unwrap();

        // Detection should publish the runtimes capability even when the test
        // database has no runtime definitions.
        let runtimes = registration.capabilities.get("runtimes").unwrap();
        let runtime_array = runtimes.as_array().unwrap();

        println!("Detected runtimes: {:?}", runtime_array);
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_sensor_worker_registration() {
        let _lock = AGENT_ENV_LOCK.lock().await;
        let (config, pool, _database) = isolated_test_context().await;
        let mut registration = SensorWorkerRegistration::new(pool, &config);

        // Test registration
        let worker_id = registration.register(&config).await.unwrap();
        assert!(worker_id > 0);
        assert_eq!(registration.worker_id(), Some(worker_id));

        // Test heartbeat
        registration.heartbeat().await.unwrap();

        // Test deregistration
        registration.deregister().await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_sensor_worker_capabilities() {
        let _lock = AGENT_ENV_LOCK.lock().await;
        let (config, pool, _database) = isolated_test_context().await;
        let mut registration = SensorWorkerRegistration::new(pool, &config);

        registration.register(&config).await.unwrap();

        // Add custom capability
        registration.add_capability("custom_feature".to_string(), json!(true));
        registration.update_capabilities().await.unwrap();

        registration.deregister().await.unwrap();
    }

    #[tokio::test]
    async fn test_inject_agent_capabilities_from_env() {
        let _lock = AGENT_ENV_LOCK.lock().await;
        let _env = EnvGuard::preserve(&[
            ATTUNE_SENSOR_AGENT_MODE_ENV,
            ATTUNE_SENSOR_AGENT_BINARY_NAME_ENV,
            ATTUNE_SENSOR_AGENT_BINARY_VERSION_ENV,
        ]);
        std::env::set_var(ATTUNE_SENSOR_AGENT_MODE_ENV, "1");
        std::env::set_var(ATTUNE_SENSOR_AGENT_BINARY_NAME_ENV, "attune-sensor-agent");
        std::env::set_var(ATTUNE_SENSOR_AGENT_BINARY_VERSION_ENV, "1.2.3");

        let mut capabilities = HashMap::new();
        SensorWorkerRegistration::inject_agent_capabilities(&mut capabilities);

        assert_eq!(capabilities.get("agent_mode"), Some(&json!(true)));
        assert_eq!(
            capabilities.get("agent_binary_name"),
            Some(&json!("attune-sensor-agent"))
        );
        assert_eq!(
            capabilities.get("agent_binary_version"),
            Some(&json!("1.2.3"))
        );
    }

    #[tokio::test]
    async fn test_env_guard_restores_values_after_panic() {
        let _lock = AGENT_ENV_LOCK.lock().await;
        let _caller_env = EnvGuard::preserve(&[ATTUNE_SENSOR_AGENT_MODE_ENV]);
        std::env::set_var(ATTUNE_SENSOR_AGENT_MODE_ENV, "caller-value");

        let result = std::panic::catch_unwind(|| {
            let _env = EnvGuard::preserve(&[ATTUNE_SENSOR_AGENT_MODE_ENV]);
            std::env::set_var(ATTUNE_SENSOR_AGENT_MODE_ENV, "test-value");
            panic!("exercise panic-safe environment restoration");
        });

        assert!(result.is_err());
        assert_eq!(
            std::env::var_os(ATTUNE_SENSOR_AGENT_MODE_ENV),
            Some(OsString::from("caller-value"))
        );
    }
}
