//! Attune Worker Service

use anyhow::Result;
use attune_common::config::Config;
use clap::Parser;
use tokio::signal::unix::{signal, SignalKind};
use tracing::info;

use attune_worker::service::WorkerService;

#[derive(Parser, Debug)]
#[command(name = "attune-worker")]
#[command(about = "Attune Worker Service - Executes automation actions", long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<String>,

    /// Worker name (overrides config)
    #[arg(short, long)]
    name: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install HMAC-only JWT crypto provider (must be before any token operations)
    attune_common::auth::install_crypto_provider();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(true)
        .init();

    let args = Args::parse();

    info!("Starting Attune Worker Service");

    // Load configuration
    if let Some(config_path) = args.config {
        std::env::set_var("ATTUNE_CONFIG", config_path);
    }

    let mut config = Config::load()?;
    config.validate()?;
    attune_common::config::set_app_default_execution_timeout_seconds(
        config.default_execution_timeout_seconds,
    );

    // Override worker name if provided via CLI
    if let Some(name) = args.name {
        if let Some(ref mut worker_config) = config.worker {
            worker_config.name = Some(name);
        } else {
            config.worker = Some(attune_common::config::WorkerConfig {
                name: Some(name),
                worker_type: None,
                runtime_id: None,
                host: None,
                port: None,
                capabilities: None,
                labels: Default::default(),
                taints: Vec::new(),
                max_concurrent_tasks: 10,
                heartbeat_interval: 30,
                task_timeout: 300,
                max_stdout_bytes: 10 * 1024 * 1024,
                max_stderr_bytes: 10 * 1024 * 1024,
                execution_log_retention_policy:
                    attune_common::models::enums::RetentionPolicyType::Days,
                execution_log_retention_limit: 7,
                shutdown_timeout: Some(30),
                stream_logs: true,
            });
        }
    }

    info!("Configuration loaded successfully");
    info!("Environment: {}", config.environment);

    // Initialize and run worker service
    let mut service = WorkerService::new(config).await?;

    info!("Attune Worker Service is ready");

    // Start the service
    service.start().await?;

    // Setup signal handlers for graceful shutdown
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

    // Stop the service and mark worker as inactive
    service.stop().await?;

    info!("Attune Worker Service shutdown complete");

    Ok(())
}
