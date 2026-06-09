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
//! - ATTUNE_MQ_URL: RabbitMQ connection URL
//! - ATTUNE_MQ_EXCHANGE: RabbitMQ exchange name (default: "attune")
//! - ATTUNE_LOG_LEVEL: Logging verbosity (default: "info")

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tracing::{error, info, warn};

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

const ALL_TIMER_TRIGGER_REFS: &[&str] = &[
    "core.intervaltimer",
    "core.crontimer",
    "core.datetimetimer",
    "core.rruletimer",
];

#[derive(Parser, Debug)]
#[command(name = "attune-core-timer-sensor")]
#[command(about = "Standalone timer sensor for Attune automation platform", long_about = None)]
struct Args {
    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// Read configuration from stdin as JSON instead of environment variables
    #[arg(long)]
    stdin_config: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    let log_level = args.log_level.parse().unwrap_or(tracing::Level::INFO);

    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .with_target(false)
        .with_thread_ids(true)
        .json()
        .init();

    info!("Starting Attune Timer Sensor");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config = if args.stdin_config {
        info!("Reading configuration from stdin");
        SensorConfig::from_stdin().await?
    } else {
        info!("Reading configuration from environment variables");
        SensorConfig::from_env()?
    };

    config.validate()?;
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
    if let Err(error) = reconcile_active_timer_rules(&api_client, &timer_manager).await {
        warn!("Initial timer rule reconciliation failed: {}", error);
    }

    // Create rule lifecycle listener
    let listener = RuleLifecycleListener::new(
        config.mq_url.clone(),
        config.mq_exchange.clone(),
        config.sensor_ref.clone(),
        api_client.clone(),
        timer_manager.clone(),
    );

    info!("Rule lifecycle listener initialized");

    // Start token refresh manager (auto-refresh when 80% of TTL elapsed)
    let refresh_manager = TokenRefreshManager::new(api_client.clone(), 0.8);
    let _refresh_handle = refresh_manager.start();
    info!("Token refresh manager started (will refresh at 80% of TTL)");

    let _reconcile_handle =
        start_rule_reconciliation_loop(api_client.clone(), timer_manager.clone());
    info!("Timer rule reconciliation loop started");

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

fn start_rule_reconciliation_loop(
    api_client: api_client::ApiClient,
    timer_manager: TimerManager,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            if let Err(error) = reconcile_active_timer_rules(&api_client, &timer_manager).await {
                warn!("Timer rule reconciliation failed: {}", error);
            }
        }
    })
}

async fn reconcile_active_timer_rules(
    api_client: &api_client::ApiClient,
    timer_manager: &TimerManager,
) -> Result<()> {
    let mut desired_rule_ids = HashSet::new();

    for trigger_ref in ALL_TIMER_TRIGGER_REFS {
        let rules = api_client
            .fetch_rules(trigger_ref)
            .await
            .with_context(|| format!("Failed to fetch active rules for trigger {}", trigger_ref))?;

        for rule in rules.into_iter().filter(|rule| rule.enabled) {
            desired_rule_ids.insert(rule.id);
            if timer_manager.has_timer(rule.id).await {
                continue;
            }

            let config = match TimerConfig::from_trigger_params(trigger_ref, rule.trigger_params) {
                Ok(config) => config,
                Err(error) => {
                    warn!(
                        "Skipping timer rule {} during reconciliation; failed to parse config: {}",
                        rule.id, error
                    );
                    continue;
                }
            };

            if let Err(error) = timer_manager.start_timer(rule.id, config).await {
                warn!(
                    "Skipping timer rule {} during reconciliation; failed to start timer: {}",
                    rule.id, error
                );
            }
        }
    }

    for rule_id in timer_manager.active_rule_ids().await {
        if !desired_rule_ids.contains(&rule_id) {
            timer_manager.stop_timer(rule_id).await;
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
