//! Attune API Service
//!
//! REST API gateway for all client interactions with the Attune platform.
//! Provides endpoints for managing packs, actions, triggers, rules, executions,
//! inquiries, and other automation components.

use anyhow::Result;
use attune_common::{
    config::Config,
    db::Database,
    metadata_cache::MetadataCache,
    mq::{Connection, Publisher, PublisherConfig},
};
use clap::Parser;
use std::sync::Arc;
use tracing::{info, warn};

use attune_api::{inquiry_timeout, postgres_listener, AppState, Server};

#[derive(Parser, Debug)]
#[command(name = "attune-api")]
#[command(about = "Attune API Service", long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<String>,

    /// Server host address
    #[arg(long)]
    host: Option<String>,

    /// Server port
    #[arg(long)]
    port: Option<u16>,
}

/// Attempt to connect to RabbitMQ and create a publisher.
/// Returns the publisher on success.
async fn try_connect_publisher(mq_url: &str) -> Result<Publisher> {
    let mq_connection = Connection::connect(mq_url).await?;

    // Setup common message queue infrastructure (exchanges and DLX)
    let mq_setup_config = attune_common::mq::MessageQueueConfig::default();
    if let Err(e) = mq_connection
        .setup_common_infrastructure(&mq_setup_config)
        .await
    {
        warn!(
            "Failed to setup common MQ infrastructure (may already exist): {}",
            e
        );
    }

    let publisher = Publisher::new(
        &mq_connection,
        PublisherConfig {
            confirm_publish: true,
            timeout_secs: 30,
            exchange: "attune.executions".to_string(),
        },
    )
    .await?;

    Ok(publisher)
}

/// Background task that keeps trying to establish the MQ publisher connection.
/// Once connected it installs the publisher into `state`, then monitors the
/// connection health and reconnects if it drops.
async fn mq_reconnect_loop(state: Arc<AppState>, mq_url: String) {
    // Retry delay sequence (seconds): 1, 2, 4, 8, 16, 30, 30, …
    let delays: &[u64] = &[1, 2, 4, 8, 16, 30];
    let mut attempt: usize = 0;

    loop {
        let delay = delays.get(attempt).copied().unwrap_or(30);

        match try_connect_publisher(&mq_url).await {
            Ok(publisher) => {
                info!(
                    "Message queue publisher connected (attempt {})",
                    attempt + 1
                );
                state.set_publisher(Arc::new(publisher)).await;
                attempt = 0; // reset backoff after a successful connect

                // Poll liveness: the publisher will error on use when the
                // underlying channel is gone.  We do a lightweight wait here so
                // we notice disconnections and attempt to reconnect.
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    if state.get_publisher().await.is_none() {
                        // Something cleared the publisher externally; re-enter
                        // the outer connect loop.
                        break;
                    }
                    // TODO: add a real health-check ping when the lapin API
                    // exposes one (e.g. channel.basic_noop).  For now a broken
                    // publisher will be detected on the first failed publish and
                    // can be cleared by the handler to trigger reconnection here.
                }
            }
            Err(e) => {
                warn!(
                    "Failed to connect to message queue (attempt {}, retrying in {}s): {}",
                    attempt + 1,
                    delay,
                    e
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install a JWT crypto provider that supports both Attune's HS tokens
    // and external RS256 OIDC identity tokens.
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();

    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(true)
        .with_level(true)
        .init();

    let args = Args::parse();

    info!("Starting Attune API Service");

    // Load configuration
    if let Some(config_path) = args.config {
        std::env::set_var("ATTUNE_CONFIG", config_path);
    }

    let config = Config::load()?;
    config.validate()?;
    attune_common::config::set_app_default_execution_timeout_seconds(
        config.default_execution_timeout_seconds,
    );
    config.warn_about_insecure_secrets();

    // SECURITY: Fail-closed check for the agent binary download endpoint.
    // If `agent.binary_dir` is configured but `agent.bootstrap_token` is not,
    // the download route would otherwise be reachable without authentication.
    // We require the operator to either set a token or remove the agent
    // section entirely.
    if let Some(ref agent_cfg) = config.agent {
        if agent_cfg.bootstrap_token.is_none() {
            anyhow::bail!(
                "agent.bootstrap_token is required when agent.binary_dir is configured. \
                 Set the token (e.g. `openssl rand -hex 32`) via ATTUNE__AGENT__BOOTSTRAP_TOKEN. \
                 To disable agent binary distribution entirely, remove the [agent] section from config."
            );
        }
    }

    info!("Configuration loaded successfully");
    info!("Environment: {}", config.environment);

    // Write sentinel file for volume auto-detection by workers/sensors
    let api_url = format!("http://{}:{}", config.server.host, config.server.port);
    if let Err(e) = attune_common::artifact_transport::detection::write_sentinel(
        &config.artifacts_dir,
        &api_url,
    ) {
        warn!("Failed to write artifact sentinel file: {e} — remote workers will default to API transport");
    }

    // Write packs sentinel for pack volume auto-detection
    if let Err(e) =
        attune_common::pack_transport::write_packs_sentinel(&config.packs_base_dir, &api_url)
    {
        warn!(
            "Failed to write packs sentinel file: {e} — remote workers will download packs via API"
        );
    }

    info!(
        "Server will bind to {}:{}",
        config.server.host, config.server.port
    );

    // Initialize database connection pool
    info!("Connecting to database...");
    let database = Database::new(&config.database).await?;
    info!("Database connection established");

    // Spawn the audit writer task. The emitter is cheap and clone-able; we
    // store it in AppState so handlers and middleware can record audit events
    // without blocking the request path.
    let audit_handle = attune_common::audit::spawn_writer(database.pool().clone());
    info!("Audit writer task started");
    let audit_emitter = audit_handle.emitter.clone();
    // Detach the writer task so it lives as long as the process.
    std::mem::forget(audit_handle.task);

    let metadata_cache = MetadataCache::best_effort_from_config(&config.metadata_cache).await;
    if metadata_cache.is_enabled() {
        info!("Metadata cache connected");
    } else {
        info!("Metadata cache disabled");
    }

    // Initialize application state (publisher starts as None)
    let state = Arc::new(AppState::new_with_audit_and_metadata_cache(
        database.pool().clone(),
        config.clone(),
        audit_emitter,
        metadata_cache,
    ));

    // Spawn background MQ reconnect loop if a message queue is configured.
    // The loop will keep retrying until it connects, then install the publisher
    // into the shared state so request handlers can use it immediately.
    if let Some(ref mq_config) = config.message_queue {
        info!("Message queue configured – starting background connection loop...");
        let mq_url = mq_config.url.clone();
        let state_clone = state.clone();
        tokio::spawn(async move {
            mq_reconnect_loop(state_clone, mq_url).await;
        });
    } else {
        warn!("Message queue not configured – executions will not be queued for processing");
    }

    info!(
        "CORS configured with {} allowed origin(s)",
        if config.server.cors_origins.is_empty() {
            "default development"
        } else {
            "custom"
        }
    );

    // Start PostgreSQL listener for SSE broadcasting
    let broadcast_tx = state.broadcast_tx.clone();
    let listener_db = database.pool().clone();
    tokio::spawn(async move {
        if let Err(e) = postgres_listener::start_postgres_listener(listener_db, broadcast_tx).await
        {
            tracing::error!("PostgreSQL listener error: {}", e);
        }
    });

    info!("PostgreSQL notification listener started");

    if state.metadata_cache.is_enabled() {
        let metadata_cache_db = database.pool().clone();
        let metadata_cache = state.metadata_cache.clone();
        tokio::spawn(async move {
            if let Err(e) = attune_common::metadata_cache::sync::start_metadata_cache_sync(
                metadata_cache_db,
                metadata_cache,
            )
            .await
            {
                tracing::error!("Metadata cache sync listener error: {}", e);
            }
        });
        info!("Metadata cache sync listener started");
    }

    let timeout_db = database.pool().clone();
    tokio::spawn(async move {
        inquiry_timeout::start_inquiry_timeout_monitor(timeout_db).await;
    });
    info!("Inquiry timeout monitor started");

    // Create and start server
    let server = Server::new(state.clone());

    info!("Attune API Service is ready");

    // Run server with graceful shutdown
    tokio::select! {
        result = server.run() => {
            if let Err(e) = result {
                tracing::error!("Server error: {}", e);
                return Err(e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    info!("Shutting down Attune API Service");

    Ok(())
}
