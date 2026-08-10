//! Attune Timer Sensor
//!
//! A standalone sensor daemon that monitors timer-based triggers and emits events
//! to the Attune platform. Each timer sensor instance manages multiple timer schedules
//! based on active rules.
//!
//! Configuration is provided via environment variables or stdin JSON:
//! - ATTUNE_API_URL: Base URL of the Attune API
//! - ATTUNE_API_TOKEN: Service account token for authentication
//! - ATTUNE_SENSOR_REF: Reference name for this sensor (e.g., "core.timer")
//! - ATTUNE_NOTIFIER_WS_URL: Notifier WebSocket endpoint for rule lifecycle deltas
//! - ATTUNE_LOG_LEVEL: Logging verbosity (default: "info")
//! - ATTUNE_LOG_FORMAT: Logging format (`json` default, `pretty` for readable local runs)

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

mod api_client;
mod config;
mod rule_listener;
mod timer_manager;
mod token_refresh;
mod types;

use config::SensorConfig;
use rule_listener::RuleLifecycleListener;
use timer_manager::TimerManager;
use token_refresh::TokenRefreshManager;
use types::TimerConfig;

#[derive(Parser, Debug)]
#[command(name = "attune-core-timer-sensor")]
#[command(about = "Standalone timer sensor for Attune automation platform", long_about = None)]
struct Args {
    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long)]
    log_level: Option<String>,

    /// Log format (json, pretty)
    #[arg(long)]
    log_format: Option<String>,

    /// Read configuration from stdin as JSON instead of environment variables
    #[arg(long)]
    stdin_config: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Load configuration
    let config = if args.stdin_config {
        SensorConfig::from_stdin().await?
    } else {
        SensorConfig::from_env()?
    };

    config.validate().await?;
    init_tracing(
        args.log_level
            .as_deref()
            .unwrap_or(config.log_level.as_str()),
        args.log_format
            .as_deref()
            .unwrap_or(config.log_format.as_str()),
    )?;

    info!("Starting Attune Timer Sensor");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));
    info!(
        config_source = if args.stdin_config { "stdin" } else { "env" },
        log_level = %args.log_level.as_deref().unwrap_or(config.log_level.as_str()),
        log_format = %args.log_format.as_deref().unwrap_or(config.log_format.as_str()),
        "Configuration loaded"
    );
    info!(
        "Configuration loaded successfully: sensor_ref={}, api_url={}",
        config.sensor_ref, config.api_url
    );

    // Create API client
    let api_client = api_client::ApiClient::new(config.api_url.clone(), config.api_token.clone());

    // Verify API connectivity
    info!("Verifying API connectivity...");
    api_client
        .health_check()
        .await
        .context("Failed to connect to Attune API")?;
    info!("API connectivity verified");

    // Create timer manager
    let timer_manager = TimerManager::new(api_client.clone(), config.sensor_ref.clone())
        .await
        .context("Failed to initialize timer manager")?;
    info!("Timer manager initialized");

    start_managed_trigger_instances(&timer_manager).await?;
    info!("Loaded managed trigger bootstrap state from ATTUNE_SENSOR_TRIGGERS");

    // Create rule lifecycle listener
    let listener = RuleLifecycleListener::new(
        config.notifier_ws_url.clone(),
        config.allow_insecure_notifier_ws,
        config.sensor_ref.clone(),
        api_client.clone(),
        timer_manager.clone(),
    );

    info!("Rule lifecycle listener initialized");

    // Start token refresh manager (auto-refresh when 80% of TTL elapsed)
    let refresh_manager = TokenRefreshManager::new(api_client.clone(), 0.8);
    let _refresh_handle = refresh_manager.start();
    info!("Token refresh manager started (will refresh at 80% of TTL)");

    // Set up graceful shutdown handler
    let timer_manager_clone = timer_manager.clone();
    let shutdown_signal = tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                info!("Shutdown signal received");
                if let Err(e) = timer_manager_clone.shutdown().await {
                    error!("Error during timer manager shutdown: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to listen for shutdown signal: {}", e);
            }
        }
    });

    // Start the listener (this will block until stopped)
    info!("Starting rule lifecycle listener...");
    match listener.start().await {
        Ok(()) => {
            info!("Rule lifecycle listener stopped gracefully");
        }
        Err(e) => {
            error!("Rule lifecycle listener error: {}", e);
            return Err(e);
        }
    }

    // Wait for shutdown to complete
    let _ = shutdown_signal.await;

    // Ensure timer manager is fully shutdown
    timer_manager.shutdown().await?;

    info!("Timer sensor has shut down gracefully");
    Ok(())
}

fn init_tracing(log_level: &str, log_format: &str) -> Result<()> {
    let env_filter = EnvFilter::try_new(log_level)
        .context("Invalid ATTUNE_LOG_LEVEL / --log-level tracing directive")?;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_thread_ids(true)
        .with_level(true);

    match log_format.trim().to_ascii_lowercase().as_str() {
        "json" => builder.json().init(),
        "pretty" => builder.pretty().init(),
        other => {
            return Err(anyhow::anyhow!(
                "unsupported ATTUNE_LOG_FORMAT / --log-format `{}`; expected `json` or `pretty`",
                other
            ));
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct ManagedTriggerInstance {
    id: i64,
    #[serde(default)]
    trigger_ref: Option<String>,
    #[serde(default)]
    config: serde_json::Value,
}

async fn start_managed_trigger_instances(timer_manager: &TimerManager) -> Result<()> {
    let Ok(raw) = std::env::var("ATTUNE_SENSOR_TRIGGERS") else {
        return Ok(());
    };
    if raw.trim().is_empty() {
        return Ok(());
    }

    let instances: Vec<ManagedTriggerInstance> = serde_json::from_str(&raw)
        .context("Failed to parse ATTUNE_SENSOR_TRIGGERS as managed trigger instances")?;
    info!(
        "Loaded {} managed trigger instance(s) from ATTUNE_SENSOR_TRIGGERS",
        instances.len()
    );

    for instance in instances {
        let trigger_ref = instance
            .trigger_ref
            .as_deref()
            .or_else(|| infer_timer_trigger_ref(&instance.config));
        let Some(trigger_ref) = trigger_ref else {
            error!(
                "Managed trigger instance {} is missing trigger_ref and cannot be inferred from config {}",
                instance.id, instance.config
            );
            continue;
        };

        match TimerConfig::from_trigger_params(trigger_ref, instance.config).with_context(|| {
            format!(
                "Failed to parse managed timer config for rule {}",
                instance.id
            )
        }) {
            Ok(timer_config) => {
                if let Err(error) = timer_manager.start_timer(instance.id, timer_config).await {
                    error!(
                        "Failed to start managed timer for rule {} (trigger {}): {}",
                        instance.id, trigger_ref, error
                    );
                }
            }
            Err(error) => {
                error!("{}", error);
            }
        }
    }

    Ok(())
}

fn infer_timer_trigger_ref(config: &serde_json::Value) -> Option<&'static str> {
    if config.get("interval").is_some() {
        Some("core.intervaltimer")
    } else if config.get("expression").is_some() {
        Some("core.crontimer")
    } else if config.get("fire_at").is_some() {
        Some("core.datetimetimer")
    } else if config.get("rule").is_some() || config.get("freq").is_some() {
        Some("core.rruletimer")
    } else {
        None
    }
}
