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
mod pack_test_processor;
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
    info!("Database: {}", mask_connection_url(&config.database.url));
    if let Some(ref mq_config) = config.message_queue {
        info!("Message Queue: {}", mask_connection_url(&mq_config.url));
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

/// Return connection metadata that is safe to include in logs.
fn mask_connection_url(url: &str) -> String {
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
        let url = "postgresql://user:password@localhost:5432/attune?sslmode=require#secret";
        let masked = mask_connection_url(url);
        assert!(!masked.contains("user"));
        assert!(!masked.contains("password"));
        assert_eq!(masked, "postgresql://localhost:5432/attune");
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
