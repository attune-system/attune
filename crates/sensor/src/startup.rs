use crate::service::SensorService;
use anyhow::Result;
use attune_common::agent_runtime_detection::DetectedRuntime;
use attune_common::config::{Config, SensorConfig};
use tracing::{error, info};

#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};

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
            passthrough_env: Vec::new(),
            notifier_ws_url: None,
            allow_insecure_notifier_ws: false,
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
    info!("Database: {}", mask_connection_url(&config.database.url));
    if let Some(ref mq_config) = config.message_queue {
        info!("Message Queue: {}", mask_connection_url(&mq_config.url));
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

    #[cfg(unix)]
    {
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
    }

    #[cfg(windows)]
    {
        use tokio::signal::windows;
        let mut sigint = windows::ctrl_c()?;
        sigint.recv().await;
        info!("Received Ctrl+C / shutdown signal");
    }

    info!("Shutting down gracefully...");

    if let Err(e) = service.stop().await {
        error!("Error during shutdown: {}", e);
    }

    Ok(())
}

/// Mask sensitive parts of connection strings for logging.
pub fn mask_connection_url(url: &str) -> String {
    let Some((scheme, remainder)) = url.split_once("://") else {
        return "<redacted connection URL>".to_string();
    };
    if scheme.is_empty()
        || !scheme.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_alphabetic() || index > 0 && "+-.0123456789".contains(ch)
        })
    {
        return "<redacted connection URL>".to_string();
    }

    let suffix_start = remainder.find(['?', '#']).unwrap_or(remainder.len());
    let without_suffix = &remainder[..suffix_start];
    let authority_end = without_suffix.find('/').unwrap_or(without_suffix.len());
    let authority = &without_suffix[..authority_end];
    let path = &without_suffix[authority_end..];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);

    if host.is_empty()
        || host.contains('@')
        || host.chars().any(char::is_whitespace)
        || path.chars().any(char::is_whitespace)
    {
        return "<redacted connection URL>".to_string();
    }

    format!("{scheme}://{host}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_url_log_value_removes_credentials_query_and_fragment() {
        let url = "amqp://user:password@rabbitmq:5672/attune?heartbeat=10#secret";
        let masked = mask_connection_url(url);
        assert!(!masked.contains("user"));
        assert!(!masked.contains("password"));
        assert_eq!(masked, "amqp://rabbitmq:5672/attune");
    }

    #[test]
    fn connection_url_log_value_keeps_safe_host_and_path() {
        let url = "postgresql://localhost:5432/attune";
        assert_eq!(mask_connection_url(url), url);
    }

    #[test]
    fn connection_url_log_value_fails_closed() {
        assert_eq!(
            mask_connection_url("not a connection URL?password=secret"),
            "<redacted connection URL>"
        );
    }
}
