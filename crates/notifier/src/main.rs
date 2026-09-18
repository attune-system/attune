//! Attune Notifier Service - Real-time notification delivery

use anyhow::Result;
use attune_common::{config::Config, observability};
use clap::Parser;
use tracing::info;

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
    info!("Database: {}", mask_connection_url(&config.database.url));

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

    let service_task = service.start();
    tokio::pin!(service_task);
    tokio::select! {
        result = &mut service_task => result?,
        result = tokio::signal::ctrl_c() => {
            result?;
            info!("Received shutdown signal");
            service.shutdown().await?;
            service_task.await?;
        }
    }

    info!("Attune Notifier Service stopped");

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
        let url = "postgresql://user:password@localhost:5432/db?sslmode=require#secret";
        assert_eq!(mask_connection_url(url), "postgresql://localhost:5432/db");
    }

    #[test]
    fn connection_url_log_value_keeps_safe_host_and_path() {
        let url = "postgresql://localhost:5432/db";
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
