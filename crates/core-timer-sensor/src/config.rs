//! Configuration module for timer sensor
//!
//! Supports loading configuration from environment variables or stdin JSON.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;

/// Sensor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorConfig {
    /// Base URL of the Attune API
    pub api_url: String,

    /// API token for authentication
    pub api_token: String,

    /// Sensor reference name (e.g., "core.timer_sensor")
    pub sensor_ref: String,

    /// Notifier websocket URL (rule lifecycle stream)
    pub notifier_ws_url: String,

    /// Log level (default: "info")
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Log format (default: "json")
    #[serde(default = "default_log_format")]
    pub log_format: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

impl SensorConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let api_url = std::env::var("ATTUNE_API_URL")
            .context("ATTUNE_API_URL environment variable is required")?;

        let api_token = std::env::var("ATTUNE_API_TOKEN")
            .context("ATTUNE_API_TOKEN environment variable is required")?;

        let sensor_ref = std::env::var("ATTUNE_SENSOR_REF")
            .context("ATTUNE_SENSOR_REF environment variable is required")?;

        let notifier_ws_url = std::env::var("ATTUNE_NOTIFIER_WS_URL")
            .context("ATTUNE_NOTIFIER_WS_URL environment variable is required")?;

        let log_level = std::env::var("ATTUNE_LOG_LEVEL").unwrap_or_else(|_| default_log_level());
        let log_format =
            std::env::var("ATTUNE_LOG_FORMAT").unwrap_or_else(|_| default_log_format());

        Ok(Self {
            api_url,
            api_token,
            sensor_ref,
            notifier_ws_url,
            log_level,
            log_format,
        })
    }

    /// Load configuration from stdin JSON
    pub async fn from_stdin() -> Result<Self> {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .context("Failed to read configuration from stdin")?;

        serde_json::from_str(&buffer).context("Failed to parse JSON configuration from stdin")
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        if self.api_url.is_empty() {
            return Err(anyhow::anyhow!("api_url cannot be empty"));
        }

        if self.api_token.is_empty() {
            return Err(anyhow::anyhow!("api_token cannot be empty"));
        }

        if self.sensor_ref.is_empty() {
            return Err(anyhow::anyhow!("sensor_ref cannot be empty"));
        }

        if self.notifier_ws_url.is_empty() {
            return Err(anyhow::anyhow!("notifier_ws_url cannot be empty"));
        }

        // Validate API URL format
        if !self.api_url.starts_with("http://") && !self.api_url.starts_with("https://") {
            return Err(anyhow::anyhow!(
                "api_url must start with http:// or https://"
            ));
        }

        if !self.notifier_ws_url.starts_with("ws://") && !self.notifier_ws_url.starts_with("wss://")
        {
            return Err(anyhow::anyhow!(
                "notifier_ws_url must start with ws:// or wss://"
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        let config = SensorConfig {
            api_url: "http://localhost:8080".to_string(),
            api_token: "test_token".to_string(),
            sensor_ref: "core.timer".to_string(),
            notifier_ws_url: "ws://localhost:8081/ws".to_string(),
            log_level: "info".to_string(),
            log_format: "json".to_string(),
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_api_url() {
        let config = SensorConfig {
            api_url: "localhost:8080".to_string(), // Missing http://
            api_token: "test_token".to_string(),
            sensor_ref: "core.timer".to_string(),
            notifier_ws_url: "ws://localhost:8081/ws".to_string(),
            log_level: "info".to_string(),
            log_format: "json".to_string(),
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_notifier_ws_url() {
        let config = SensorConfig {
            api_url: "http://localhost:8080".to_string(),
            api_token: "test_token".to_string(),
            sensor_ref: "core.timer".to_string(),
            notifier_ws_url: "localhost:8081/ws".to_string(), // Missing ws://
            log_level: "info".to_string(),
            log_format: "json".to_string(),
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_deserialization() {
        let json = r#"{
            "api_url": "http://localhost:8080",
            "api_token": "test_token",
            "sensor_ref": "core.timer",
            "notifier_ws_url": "ws://localhost:8081/ws"
        }"#;

        let config: SensorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.api_url, "http://localhost:8080");
        assert_eq!(config.api_token, "test_token");
        assert_eq!(config.sensor_ref, "core.timer");
        assert_eq!(config.notifier_ws_url, "ws://localhost:8081/ws");
        assert_eq!(config.log_level, "info"); // Default
        assert_eq!(config.log_format, "json"); // Default
    }

    #[test]
    fn test_config_deserialization_with_optionals() {
        let json = r#"{
            "api_url": "http://localhost:8080",
            "api_token": "test_token",
            "sensor_ref": "core.timer",
            "notifier_ws_url": "wss://notify.example/ws",
            "log_level": "debug",
            "log_format": "pretty"
        }"#;

        let config: SensorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.notifier_ws_url, "wss://notify.example/ws");
        assert_eq!(config.log_level, "debug");
        assert_eq!(config.log_format, "pretty");
    }
}
