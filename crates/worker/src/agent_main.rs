//! Attune Universal Worker Agent
//!
//! This is the entrypoint for the universal worker agent binary (`attune-agent`).
//! Unlike the standard `attune-worker` binary which requires explicit runtime
//! configuration, the agent automatically detects available interpreters in the
//! container environment and configures itself accordingly.
//!
//! ## Usage
//!
//! The agent is designed to be injected into any container image. On startup it:
//!
//! 1. Probes the system for available interpreters (python3, node, bash, etc.)
//! 2. Sets `ATTUNE_WORKER_RUNTIMES` based on what it finds
//! 3. Loads configuration (env vars are the primary config source)
//! 4. Initializes and runs the standard `WorkerService`
//!
//! ## Configuration
//!
//! Environment variables (primary):
//! - `ATTUNE__DATABASE__URL` — PostgreSQL connection string
//! - `ATTUNE__MESSAGE_QUEUE__URL` — RabbitMQ connection string
//! - `ATTUNE_WORKER_RUNTIMES` — Override auto-detection with explicit runtime list
//! - `ATTUNE_CONFIG` — Path to optional config YAML file
//!
//! CLI arguments:
//! - `--config` / `-c` — Path to configuration file (optional)
//! - `--name` / `-n` — Worker name override
//! - `--detect-only` — Run runtime detection, print results, and exit

use anyhow::Result;
use attune_common::{
    agent_bootstrap::{bootstrap_runtime_env, print_detect_only_report},
    config::Config,
    observability,
};
use clap::Parser;
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use tracing::{info, warn};

use attune_worker::dynamic_runtime::auto_register_detected_runtimes;
use attune_worker::runtime_detect::DetectedRuntime;
use attune_worker::service::WorkerService;

#[derive(Parser, Debug)]
#[command(name = "attune-agent")]
#[command(
    version,
    about = "Attune Universal Worker Agent - Injected into any container to auto-detect and execute actions",
    long_about = "The Attune Agent automatically discovers available runtime interpreters \
                  in the current environment and registers as a worker capable of executing \
                  actions for those runtimes. It is designed to be injected into arbitrary \
                  container images without requiring manual runtime configuration."
)]
struct Args {
    /// Path to configuration file (optional — env vars are the primary config source)
    #[arg(short, long)]
    config: Option<String>,

    /// Worker name (overrides config and auto-generated name)
    #[arg(short, long)]
    name: Option<String>,

    /// Run runtime detection, print results, and exit without starting the worker
    #[arg(long)]
    detect_only: bool,
}

fn main() -> Result<()> {
    // Install HMAC-only JWT crypto provider (must be before any token operations)
    attune_common::auth::install_crypto_provider();

    let args = Args::parse();

    // Safe: no async runtime or worker threads are running yet.
    std::env::set_var("ATTUNE_AGENT_MODE", "true");
    std::env::set_var("ATTUNE_AGENT_BINARY_NAME", "attune-agent");
    std::env::set_var("ATTUNE_AGENT_BINARY_VERSION", env!("CARGO_PKG_VERSION"));

    // --- Set config path env var (synchronous, before tokio runtime) ---
    if let Some(ref config_path) = args.config {
        // Safe: no other threads are running yet (tokio runtime not started).
        std::env::set_var("ATTUNE_CONFIG", config_path);
    }

    if args.detect_only {
        let _ = observability::init_tracing(None, None)?;
        let bootstrap = bootstrap_runtime_env("ATTUNE_WORKER_RUNTIMES");
        print_detect_only_report("ATTUNE_WORKER_RUNTIMES", &bootstrap);
        return Ok(());
    }

    let config = Config::load()?;
    config.validate()?;
    let tracing_init = observability::init_tracing_from_config(&config, None)?;
    info!(
        level = %tracing_init.resolved.level_directive,
        level_source = tracing_init.resolved.level_source.as_str(),
        format = tracing_init.resolved.format.as_str(),
        initialized = tracing_init.initialized,
        "Tracing initialized"
    );
    info!("Starting Attune Universal Worker Agent");
    info!("Agent binary: attune-agent {}", env!("CARGO_PKG_VERSION"));

    let bootstrap = bootstrap_runtime_env("ATTUNE_WORKER_RUNTIMES");
    let agent_detected_runtimes = bootstrap.detected_runtimes.clone();

    // --- Build the tokio runtime and run the async portion ---
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async_main(args.name, config, agent_detected_runtimes))
}

/// The async portion of the agent entrypoint. Called from `main()` via
/// `runtime.block_on()` after all environment variable mutations are complete.
async fn async_main(
    name: Option<String>,
    mut config: Config,
    agent_detected_runtimes: Option<Vec<DetectedRuntime>>,
) -> Result<()> {
    // Override worker name if provided via CLI
    if let Some(name) = name {
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

    // --- Phase 2b: Dynamic runtime registration ---
    //
    // Before creating the WorkerService (which loads runtimes from the DB into
    // its runtime registry), ensure that every detected runtime has a
    // corresponding entry in the database. This handles the case where the
    // agent detects a runtime (e.g., Ruby) that has a template in the core
    // pack but hasn't been explicitly loaded by this agent before.
    if let Some(ref detected) = agent_detected_runtimes {
        info!(
            "Ensuring {} detected runtime(s) are registered in the database...",
            detected.len()
        );

        // We need a temporary DB connection for dynamic registration.
        // WorkerService::new() will create its own connection, so this is
        // a short-lived pool just for the registration step.
        let db = attune_common::db::Database::new(&config.database).await?;
        let pool = db.pool().clone();

        match auto_register_detected_runtimes(&pool, detected).await {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Dynamic registration complete: {} new runtime(s) added to database",
                        count
                    );
                } else {
                    info!("Dynamic registration: all detected runtimes already in database");
                }
            }
            Err(e) => {
                warn!(
                    "Dynamic runtime registration failed (non-fatal, continuing): {}",
                    e
                );
            }
        }
    }

    // --- Phase 3: Initialize and run the worker service ---
    let service = WorkerService::new(config).await?;

    // If we auto-detected runtimes, pass them to the worker service so that
    // registration includes the full `detected_interpreters` capability
    // (binary paths + versions) and the `agent_mode` flag.
    let mut service = if let Some(detected) = agent_detected_runtimes {
        info!(
            "Passing {} detected runtime(s) to worker registration",
            detected.len()
        );
        service.with_detected_runtimes(detected)
    } else {
        service
    };

    info!("Attune Agent is ready");

    service.start().await?;

    // Setup signal handlers for graceful shutdown
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
        let mut sigint = tokio::signal::windows::ctrl_c()?;
        sigint.recv().await;
        info!("Received Ctrl+C / shutdown signal");
    }

    info!("Shutting down gracefully...");

    service.stop().await?;

    info!("Attune Agent shutdown complete");

    Ok(())
}
