//! Attune Executor Service
//!
//! The Executor is the core orchestration engine that:
//! - Processes enforcements from triggered rules
//! - Schedules executions to workers
//! - Manages execution lifecycle
//! - Enforces execution policies
//! - Orchestrates workflows
//! - Handles human-in-the-loop inquiries

mod completion_listener;
mod dead_letter_handler;
mod enforcement_processor;
mod event_processor;
mod execution_manager;
mod inquiry_handler;
mod policy_enforcer;
mod queue_dispatcher;
mod queue_manager;
mod retry_manager;
mod scheduler;
mod service;
mod timeout_monitor;
mod work_queue_events;
mod worker_health;
mod workflow;

use anyhow::Result;
use attune_common::{config::Config, observability};
use clap::Parser;
use service::ExecutorService;
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(name = "attune-executor")]
#[command(about = "Attune Executor Service - Execution orchestration and scheduling", long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<String>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long)]
    log_level: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install HMAC-only JWT crypto provider (must be before any token operations)
    attune_common::auth::install_crypto_provider();

    let args = Args::parse();

    // Load configuration
    if let Some(ref config_path) = args.config {
        std::env::set_var("ATTUNE_CONFIG", config_path);
    }

    let config = Config::load()?;
    config.validate()?;
    let tracing_init = observability::init_tracing_from_config(&config, args.log_level.as_deref())?;
    attune_common::config::set_app_default_execution_timeout_seconds(
        config.default_execution_timeout_seconds,
    );

    info!(
        level = %tracing_init.resolved.level_directive,
        level_source = tracing_init.resolved.level_source.as_str(),
        format = tracing_init.resolved.format.as_str(),
        initialized = tracing_init.initialized,
        "Tracing initialized"
    );
    info!("Starting Attune Executor Service");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));

    info!("Configuration loaded successfully");
    info!("Environment: {}", config.environment);
    info!("Database: {}", mask_connection_string(&config.database.url));
    if let Some(ref mq_config) = config.message_queue {
        info!("Message Queue: {}", mask_connection_string(&mq_config.url));
    }

    // Create executor service
    let service = ExecutorService::new(config).await?;

    info!("Executor Service initialized successfully");

    // Set up graceful shutdown handler
    let service_clone = service.clone();
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!("Failed to listen for shutdown signal: {}", e);
        } else {
            info!("Shutdown signal received");
            if let Err(e) = service_clone.stop().await {
                error!("Error during shutdown: {}", e);
            }
        }
    });

    // Start the service
    info!("Starting Executor Service components...");
    if let Err(e) = service.start().await {
        error!("Executor Service error: {}", e);
        return Err(e);
    }

    info!("Executor Service has shut down gracefully");

    Ok(())
}

/// Mask sensitive parts of connection strings for logging
fn mask_connection_string(url: &str) -> String {
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
