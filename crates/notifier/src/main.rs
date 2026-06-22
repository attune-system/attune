//! Attune Notifier Service - Real-time notification delivery

use anyhow::Result;
use attune_common::{config::Config, observability};
use clap::Parser;
use tracing::{error, info};

mod postgres_listener;
mod service;
mod subscriber_manager;
mod websocket_server;

use service::NotifierService;

#[derive(Parser, Debug)]
#[command(name = "attune-notifier")]
#[command(about = "Attune Notifier Service - Real-time notifications", long_about = None)]
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

    info!(
        level = %tracing_init.resolved.level_directive,
        level_source = tracing_init.resolved.level_source.as_str(),
        format = tracing_init.resolved.format.as_str(),
        initialized = tracing_init.initialized,
        "Tracing initialized"
    );
    info!("Starting Attune Notifier Service");

    info!("Configuration loaded successfully");
    info!("Environment: {}", config.environment);
    info!("Database: {}", mask_password(&config.database.url));

    let notifier_config = config
        .notifier
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Notifier configuration not found in config file"))?;

    info!(
        "Listening on: {}:{}",
        notifier_config.host, notifier_config.port
    );

    // Create and start the notifier service
    let service = NotifierService::new(config).await?;

    info!("Notifier Service initialized successfully");

    // Set up graceful shutdown handler
    let service_clone = std::sync::Arc::new(service);
    let service_for_shutdown = service_clone.clone();

    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        info!("Received shutdown signal");

        if let Err(e) = service_for_shutdown.shutdown().await {
            error!("Error during shutdown: {}", e);
        }
    });

    // Start the service (blocks until shutdown)
    if let Err(e) = service_clone.start().await {
        error!("Notifier service error: {}", e);
        return Err(e);
    }

    info!("Attune Notifier Service stopped");

    Ok(())
}

/// Mask password in database URL for logging
fn mask_password(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        if let Some(colon_pos) = url[..at_pos].rfind(':') {
            let mut masked = url.to_string();
            masked.replace_range(colon_pos + 1..at_pos, "****");
            return masked;
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_password() {
        let url = "postgresql://user:password@localhost:5432/db";
        let masked = mask_password(url);
        assert_eq!(masked, "postgresql://user:****@localhost:5432/db");
    }

    #[test]
    fn test_mask_password_no_password() {
        let url = "postgresql://localhost:5432/db";
        let masked = mask_password(url);
        assert_eq!(masked, url);
    }
}
