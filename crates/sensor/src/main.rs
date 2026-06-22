//! Attune Sensor Service
//!
//! The Sensor Service monitors for trigger conditions and generates events.

use anyhow::Result;
use attune_common::{config::Config, observability};
use attune_sensor::startup::{log_config_details, run_sensor_service, set_config_path};
use clap::Parser;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "attune-sensor")]
#[command(about = "Attune Sensor Service - Event monitoring and generation", long_about = None)]
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
    attune_common::auth::install_crypto_provider();

    let args = Args::parse();
    set_config_path(args.config.as_deref());

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
    info!("Starting Attune Sensor Service");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));

    log_config_details(&config);
    run_sensor_service(config, None, "Attune Sensor Service is ready").await?;
    info!("Attune Sensor Service shutdown complete");

    Ok(())
}
