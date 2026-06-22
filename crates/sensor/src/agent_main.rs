//! Attune Universal Sensor Agent.

use anyhow::Result;
use attune_common::{
    agent_bootstrap::{bootstrap_runtime_env, print_detect_only_report},
    agent_runtime_detection::DetectedRuntime,
    config::Config,
    observability,
};
use attune_sensor::startup::{
    apply_sensor_name_override, log_config_details, run_sensor_service, set_config_path,
};
use clap::Parser;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "attune-sensor-agent")]
#[command(
    version,
    about = "Attune Universal Sensor Agent - Injected into runtime containers to auto-detect sensor runtimes"
)]
struct Args {
    /// Path to configuration file (optional)
    #[arg(short, long)]
    config: Option<String>,

    /// Sensor worker name override
    #[arg(short, long)]
    name: Option<String>,

    /// Run runtime detection, print results, and exit
    #[arg(long)]
    detect_only: bool,
}

fn main() -> Result<()> {
    attune_common::auth::install_crypto_provider();

    let args = Args::parse();

    // Safe: no async runtime or worker threads are running yet.
    std::env::set_var("ATTUNE_SENSOR_AGENT_MODE", "true");
    std::env::set_var("ATTUNE_SENSOR_AGENT_BINARY_NAME", "attune-sensor-agent");
    std::env::set_var(
        "ATTUNE_SENSOR_AGENT_BINARY_VERSION",
        env!("CARGO_PKG_VERSION"),
    );

    set_config_path(args.config.as_deref());

    if args.detect_only {
        let _ = observability::init_tracing(None, None)?;
        let bootstrap = bootstrap_runtime_env("ATTUNE_SENSOR_RUNTIMES");
        print_detect_only_report("ATTUNE_SENSOR_RUNTIMES", &bootstrap);
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
    info!("Starting Attune Universal Sensor Agent");
    info!(
        "Agent binary: attune-sensor-agent {}",
        env!("CARGO_PKG_VERSION")
    );

    let bootstrap = bootstrap_runtime_env("ATTUNE_SENSOR_RUNTIMES");
    let agent_detected_runtimes = bootstrap.detected_runtimes.clone();

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async_main(args.name, config, agent_detected_runtimes))
}

async fn async_main(
    name: Option<String>,
    mut config: Config,
    agent_detected_runtimes: Option<Vec<DetectedRuntime>>,
) -> Result<()> {
    if let Some(name) = name {
        apply_sensor_name_override(&mut config, name);
    }

    log_config_details(&config);
    run_sensor_service(
        config,
        agent_detected_runtimes,
        "Attune Sensor Agent is ready",
    )
    .await?;
    info!("Attune Sensor Agent shutdown complete");

    Ok(())
}
