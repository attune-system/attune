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

    /// Allow a non-loopback plaintext notifier websocket.
    #[serde(default)]
    pub allow_insecure_notifier_ws: bool,

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
        let allow_insecure_notifier_ws = std::env::var("ATTUNE_ALLOW_INSECURE_NOTIFIER_WS")
            .or_else(|_| std::env::var("ATTUNE__SENSOR__ALLOW_INSECURE_NOTIFIER_WS"))
            .ok()
            .map(|value| parse_bool(&value))
            .transpose()?
            .unwrap_or(false);

        let log_level = std::env::var("ATTUNE_LOG_LEVEL").unwrap_or_else(|_| default_log_level());
        let log_format =
            std::env::var("ATTUNE_LOG_FORMAT").unwrap_or_else(|_| default_log_format());

        Ok(Self {
            api_url,
            api_token,
            sensor_ref,
            notifier_ws_url,
            allow_insecure_notifier_ws,
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
    pub async fn validate(&self) -> Result<()> {
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

        let notifier_url = reqwest::Url::parse(&self.notifier_ws_url)
            .context("notifier_ws_url is not a valid URL")?;
        if !notifier_url.username().is_empty() || notifier_url.password().is_some() {
            return Err(anyhow::anyhow!(
                "notifier_ws_url must not contain credentials"
            ));
        }
        let host = notifier_url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("notifier_ws_url must include a host"))?;
        match notifier_url.scheme() {
            "wss" => {}
            "ws" if self.allow_insecure_notifier_ws => {}
            "ws" => {
                let port = notifier_url
                    .port_or_known_default()
                    .ok_or_else(|| anyhow::anyhow!("notifier_ws_url must include a port"))?;
                let lookup_host = host
                    .strip_prefix('[')
                    .and_then(|host| host.strip_suffix(']'))
                    .unwrap_or(host);
                let addresses = tokio::net::lookup_host((lookup_host, port))
                    .await
                    .with_context(|| format!("failed to resolve notifier websocket host '{host}'"))?
                    .collect::<Vec<_>>();
                if addresses.is_empty()
                    || addresses.iter().any(|address| !address.ip().is_loopback())
                {
                    return Err(anyhow::anyhow!(
                        "non-loopback notifier_ws_url requires wss:// or allow_insecure_notifier_ws=true"
                    ));
                }
            }
            _ => return Err(anyhow::anyhow!("notifier_ws_url must use wss:// or ws://")), // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Validation error names the only accepted websocket schemes; it is not an endpoint.
        }

        Ok(())
    }
}

fn parse_bool(value: &str) -> Result<bool> {
    value
        .parse::<bool>()
        .with_context(|| format!("invalid boolean value {value:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_config_validation() {
        let config = SensorConfig {
            api_url: "http://localhost:8080".to_string(),
            api_token: "test_token".to_string(),
            sensor_ref: "core.timer".to_string(),
            notifier_ws_url: "ws://localhost:8081/ws".to_string(), // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test for loopback-only plaintext.
            allow_insecure_notifier_ws: false,
            log_level: "info".to_string(),
            log_format: "json".to_string(),
        };

        assert!(config.validate().await.is_ok());
    }

    #[tokio::test]
    async fn test_config_validation_invalid_api_url() {
        let config = SensorConfig {
            api_url: "localhost:8080".to_string(), // Missing http://
            api_token: "test_token".to_string(),
            sensor_ref: "core.timer".to_string(),
            notifier_ws_url: "ws://localhost:8081/ws".to_string(), // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test fixture using a loopback endpoint.
            allow_insecure_notifier_ws: false,
            log_level: "info".to_string(),
            log_format: "json".to_string(),
        };

        assert!(config.validate().await.is_err());
    }

    #[tokio::test]
    async fn test_config_validation_invalid_notifier_ws_url() {
        let config = SensorConfig {
            api_url: "http://localhost:8080".to_string(),
            api_token: "test_token".to_string(),
            sensor_ref: "core.timer".to_string(),
            notifier_ws_url: "localhost:8081/ws".to_string(), // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Deliberately malformed test fixture verifies rejection of a missing websocket scheme.
            allow_insecure_notifier_ws: false,
            log_level: "info".to_string(),
            log_format: "json".to_string(),
        };

        assert!(config.validate().await.is_err());
    }

    #[test]
    fn test_config_deserialization() {
        // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Deserialization fixture uses a loopback-only endpoint.
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
        assert_eq!(config.notifier_ws_url, "ws://localhost:8081/ws"); // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Assertion for the loopback-only fixture.
        assert!(!config.allow_insecure_notifier_ws);
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

    #[tokio::test]
    async fn test_non_loopback_plaintext_requires_explicit_opt_in() {
        let mut config: SensorConfig = serde_json::from_value(serde_json::json!({
            "api_url": "https://api.example",
            "api_token": "token",
            "sensor_ref": "core.timer",
            "notifier_ws_url": "ws://notifier:8081/ws" // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test for explicit insecure opt-in.
        }))
        .unwrap();

        assert!(config.validate().await.is_err());
        config.allow_insecure_notifier_ws = true;
        assert!(config.validate().await.is_ok());
    }

    #[tokio::test]
    async fn test_notifier_url_rejects_credentials_and_missing_host() {
        let mut config: SensorConfig = serde_json::from_value(serde_json::json!({
            "api_url": "https://api.example",
            "api_token": "token",
            "sensor_ref": "core.timer",
            "notifier_ws_url": "wss://user:secret@notify.example/ws"
        }))
        .unwrap();
        assert!(config.validate().await.is_err());

        config.notifier_ws_url = "wss://[::1".to_string();
        assert!(config.validate().await.is_err());
    }

    #[tokio::test]
    async fn test_plaintext_localhost_resolves_only_to_loopback_addresses() {
        let config: SensorConfig = serde_json::from_value(serde_json::json!({
            "api_url": "https://api.example",
            "api_token": "token",
            "sensor_ref": "core.timer",
            "notifier_ws_url": "ws://localhost:8081/ws" // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test that all localhost DNS results are loopback.
        }))
        .unwrap();

        assert!(config.validate().await.is_ok());
    }
}
