use crate::service::SensorService;
use anyhow::Result;
use attune_common::agent_runtime_detection::DetectedRuntime;
use attune_common::config::{Config, SensorConfig};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info};

pub fn set_config_path(config_path: Option<&str>) {
    if let Some(config_path) = config_path {
        std::env::set_var("ATTUNE_CONFIG", config_path);
    }
}

pub fn apply_sensor_name_override(config: &mut Config, name: String) {
    if let Some(ref mut sensor_config) = config.sensor {
        sensor_config.worker_name = Some(name);
    } else {
        config.sensor = Some(SensorConfig {
            worker_name: Some(name),
            host: None,
            capabilities: None,
            labels: Default::default(),
            taints: Vec::new(),
            max_concurrent_sensors: None,
            heartbeat_interval: 30,
            poll_interval: 30,
            sensor_timeout: 30,
            shutdown_timeout: 30,
        });
    }
}

pub fn log_config_details(config: &Config) {
    info!("Configuration loaded successfully");
    info!("Environment: {}", config.environment);
    info!("Database: {}", mask_connection_string(&config.database.url));
    if let Some(ref mq_config) = config.message_queue {
        info!("Message Queue: {}", mask_connection_string(&mq_config.url));
    }
}

pub async fn run_sensor_service(
    config: Config,
    detected_runtimes: Option<Vec<DetectedRuntime>>,
    ready_message: &str,
) -> Result<()> {
    let mut service = SensorService::new(config).await?;
    if let Some(detected) = detected_runtimes {
        service = service.with_detected_runtimes(detected).await;
    }

    info!("Sensor Service initialized successfully");
    info!("Starting Sensor Service components...");
    service.start().await?;
    info!("{}", ready_message);

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::select! {
        _ = sigint.recv() => {
            info!("Received SIGINT signal");
        }
        _ = sigterm.recv() => {
            info!("Received SIGTERM signal");
        }
    }

    info!("Shutting down gracefully...");

    if let Err(e) = service.stop().await {
        error!("Error during shutdown: {}", e);
    }

    Ok(())
}

/// Mask sensitive parts of connection strings for logging.
pub fn mask_connection_string(url: &str) -> String {
    if let Some(at_pos) = url.find('@') {
        if let Some(proto_end) = url.find("://") {
            let protocol = &url[..proto_end + 3];
            let host_and_path = &url[at_pos..];
            return format!("{}***:***{}", protocol, host_and_path);
        }
    }
    "***:***@***".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_connection_string() {
        let url = "postgresql://user:password@localhost:5432/attune";
        let masked = mask_connection_string(url);
        assert!(!masked.contains("user"));
        assert!(!masked.contains("password"));
        assert!(masked.contains("@localhost"));
    }

    #[test]
    fn test_mask_connection_string_no_credentials() {
        let url = "postgresql://localhost:5432/attune";
        let masked = mask_connection_string(url);
        assert_eq!(masked, "***:***@***");
    }
}
