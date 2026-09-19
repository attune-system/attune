//! Attune API Service
//!
//! REST API gateway for all client interactions with the Attune platform.
//! Provides endpoints for managing packs, actions, triggers, rules, executions,
//! inquiries, and other automation components.

use anyhow::{bail, Result};
use attune_common::{
    config::Config,
    db::Database,
    mq::{
        routing_keys, Connection, IdentityAuthorizationChangedPayload, MessageEnvelope,
        MessageType, PermissionSetChangedPayload, Publisher, PublisherConfig,
    },
    observability,
    repositories::pack_install::PackInstallRepository,
    repositories::platform_catalog::PlatformCatalogRepository,
};
use clap::Parser;
use std::sync::Arc;
use tracing::{info, warn};

const ABANDONED_PACK_STAGING_TTL: std::time::Duration = std::time::Duration::from_secs(8 * 60 * 60);

fn is_abandoned_pack_staging(file_name: &str, age: Option<std::time::Duration>) -> bool {
    file_name.starts_with('.')
        && file_name.ends_with(".staging")
        && age.is_some_and(|age| age >= ABANDONED_PACK_STAGING_TTL)
}

use attune_api::{
    inquiry_timeout, pack_release_upgrade::upgrade_legacy_pack_releases, postgres_listener,
    AppState, Server,
};

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

    /// Apply embedded database migrations before exiting
    #[arg(long)]
    migrate: bool,

    /// Upgrade legacy pack releases from the configured packs directory and exit
    #[arg(
        long = "upgrade-pack-releases",
        long_help = "Upgrade legacy pack releases from the configured packs directory and exit.\n\
                     Assumes migrations are already applied unless --migrate is also passed."
    )]
    upgrade_pack_releases: bool,
}

fn report_legacy_pack_upgrade(
    report: &attune_api::pack_release_upgrade::PackReleaseUpgradeReport,
    fail_on_error: bool,
) -> Result<()> {
    for pack_ref in &report.upgraded {
        info!(pack_ref, "Upgraded legacy pack to an immutable release");
    }
    for failure in &report.failures {
        tracing::error!(
            pack_ref = failure.pack_ref,
            error = failure.error,
            "Legacy pack upgrade failed"
        );
    }
    info!(
        upgraded = report.upgraded.len(),
        failures = report.failures.len(),
        "Legacy pack upgrade completed"
    );

    if fail_on_error && !report.failures.is_empty() {
        let failures = report
            .failures
            .iter()
            .map(|failure| format!("{}: {}", failure.pack_ref, failure.error))
            .collect::<Vec<_>>()
            .join("; ");
        bail!(
            "failed to upgrade {} legacy pack(s): {failures}",
            report.failures.len()
        );
    }

    Ok(())
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

async fn authz_metadata_invalidation_loop(mq_url: String) {
    loop {
        match Connection::connect(&mq_url).await {
            Ok(connection) => {
                let setup_config = attune_common::mq::MessageQueueConfig::default();
                if let Err(error) = connection.setup_common_infrastructure(&setup_config).await {
                    warn!(
                        "Failed to setup MQ infrastructure for authz invalidation consumer: {}",
                        error
                    );
                }

                match connection
                    .create_ephemeral_topic_consumer(
                        "attune.metadata",
                        &[
                            routing_keys::METADATA_PERMISSION_SET_CHANGED,
                            routing_keys::METADATA_IDENTITY_AUTHORIZATION_CHANGED,
                            routing_keys::METADATA_PACK_CHANGED,
                        ],
                        "api.authz.metadata.invalidation",
                        32,
                    )
                    .await
                {
                    Ok(consumer) => {
                        let consume_result = consumer
                            // The queue is server-named and auto-delete, so this
                            // outer loop must recreate its topology after loss.
                            .consume_once_with_handler(
                                |envelope: MessageEnvelope<serde_json::Value>| async move {
                                    match envelope.message_type {
                                        MessageType::PermissionSetChanged => {
                                            let payload: PermissionSetChangedPayload =
                                                serde_json::from_value(envelope.payload).map_err(
                                                    |e| {
                                                        attune_common::mq::MqError::Deserialization(
                                                            format!(
                                                            "Failed to parse PermissionSetChanged payload: {}",
                                                            e
                                                        ),
                                                        )
                                                    },
                                                )?;
                                            attune_api::authz::AuthorizationService::handle_permission_set_metadata_change(payload).await;
                                        }
                                        MessageType::PackChanged => {
                                            attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
                                        }
                                        MessageType::IdentityAuthorizationChanged => {
                                            let payload: IdentityAuthorizationChangedPayload =
                                                serde_json::from_value(envelope.payload).map_err(
                                                    |e| {
                                                        attune_common::mq::MqError::Deserialization(
                                                            format!(
                                                            "Failed to parse IdentityAuthorizationChanged payload: {}",
                                                            e
                                                        ),
                                                        )
                                                    },
                                                )?;
                                            attune_api::authz::AuthorizationService::handle_identity_authorization_metadata_change(payload).await;
                                        }
                                        _ => {}
                                    }
                                    Ok(())
                                },
                            )
                            .await;
                        if let Err(error) = consume_result {
                            warn!(
                                "Authz metadata invalidation consumer ended with error: {}",
                                error
                            );
                        }
                    }
                    Err(error) => {
                        warn!(
                            "Failed to create authz metadata invalidation consumer: {}",
                            error
                        );
                    }
                }
            }
            Err(error) => {
                warn!(
                    "Failed to connect MQ for authz metadata invalidation consumer: {}",
                    error
                );
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install a JWT crypto provider that supports both Attune's HS tokens
    // and external RS256 OIDC identity tokens.
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();

    let args = Args::parse();

    // Load configuration
    if let Some(ref config_path) = args.config {
        std::env::set_var("ATTUNE_CONFIG", config_path);
    }

    let config = Config::load()?;
    if !args.migrate || args.upgrade_pack_releases {
        config.validate()?;
    }
    let tracing_init = observability::init_tracing_from_config(&config, None)?;
    info!(
        level = %tracing_init.resolved.level_directive,
        level_source = tracing_init.resolved.level_source.as_str(),
        format = tracing_init.resolved.format.as_str(),
        initialized = tracing_init.initialized,
        "Tracing initialized"
    );
    info!("Starting Attune API Service");
    attune_common::config::set_app_default_execution_timeout_seconds(
        config.default_execution_timeout_seconds,
    );

    if args.migrate || args.upgrade_pack_releases {
        info!("Connecting to database...");
        let database = Database::new(&config.database).await?;

        if args.migrate {
            database.migrate().await?;
        }
        PlatformCatalogRepository::reconcile(database.pool()).await?;

        let upgrade_result = if args.upgrade_pack_releases {
            let state = AppState::new(database.pool().clone(), config.clone());
            info!("Checking blob storage exact-version operations");
            state.blob_store.preflight().await?;
            info!("Blob storage preflight passed");

            let packs_dir = std::path::Path::new(&config.packs_base_dir);
            info!(
                packs_dir = %packs_dir.display(),
                "Upgrading legacy pack releases"
            );
            let report =
                upgrade_legacy_pack_releases(database.pool(), state.blob_store.as_ref(), packs_dir)
                    .await?;
            report_legacy_pack_upgrade(&report, true)
        } else {
            Ok(())
        };

        database.close().await;
        return upgrade_result;
    }

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
    PlatformCatalogRepository::reconcile(database.pool()).await?;
    info!("Platform catalog reconciled");

    // Spawn the audit writer task. The emitter is cheap and clone-able; we
    // store it in AppState so handlers and middleware can record audit events
    // without blocking the request path.
    let audit_handle = attune_common::audit::spawn_writer(database.pool().clone());
    info!("Audit writer task started");
    let audit_emitter = audit_handle.emitter.clone();
    // Detach the writer task so it lives as long as the process.
    std::mem::forget(audit_handle.task);

    // Initialize application state (publisher starts as None)
    let state = Arc::new(AppState::new_with_audit(
        database.pool().clone(),
        config.clone(),
        audit_emitter,
    ));
    info!("Checking blob storage exact-version operations");
    state.blob_store.preflight().await?;
    info!("Blob storage preflight passed");

    let upgrade = upgrade_legacy_pack_releases(
        &state.db,
        state.blob_store.as_ref(),
        std::path::Path::new(&state.config.packs_base_dir),
    )
    .await?;
    // Keep the basic API available so bootstrap or an operator can repair a
    // pack whose source bytes are unavailable. Readiness remains closed.
    report_legacy_pack_upgrade(&upgrade, false)?;

    let stale_install_pool = database.pool().clone();
    let stale_install_packs_dir = config.packs_base_dir.clone();
    tokio::spawn(async move {
        let repository = PackInstallRepository::new(stale_install_pool);
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            match repository.fail_stale_active().await {
                Ok(installs) => {
                    for install in installs {
                        let candidate = std::path::Path::new(&stale_install_packs_dir)
                            .join(format!(".pack-test-{}", install.id));
                        if let Err(error) = tokio::fs::remove_dir_all(&candidate).await {
                            if error.kind() != std::io::ErrorKind::NotFound {
                                warn!(path = %candidate.display(), %error, "Failed to remove stale pack test candidate");
                            }
                        }
                    }
                }
                Err(error) => warn!(%error, "Failed to recover stale pack installs"),
            }

            let mut candidates = match tokio::fs::read_dir(&stale_install_packs_dir).await {
                Ok(candidates) => candidates,
                Err(error) => {
                    warn!(%error, "Failed to scan pack test candidates");
                    continue;
                }
            };
            while let Ok(Some(entry)) = candidates.next_entry().await {
                let file_name = entry.file_name();
                let Some(file_name) = file_name.to_str() else {
                    continue;
                };
                let age = entry
                    .metadata()
                    .await
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| modified.elapsed().ok());
                if is_abandoned_pack_staging(file_name, age) {
                    if let Err(error) = tokio::fs::remove_dir_all(entry.path()).await {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            warn!(path = %entry.path().display(), %error, "Failed to remove abandoned pack activation staging directory");
                        }
                    }
                    continue;
                }
                let Some(install_id) = file_name
                    .strip_prefix(".pack-test-")
                    .and_then(|id| id.parse::<i64>().ok())
                else {
                    continue;
                };
                match repository.find_by_id(install_id).await {
                    Ok(Some(install))
                        if attune_common::repositories::pack_install_is_terminal(
                            &install.status,
                        ) || install.status == "activating" =>
                    {
                        if let Err(error) = tokio::fs::remove_dir_all(entry.path()).await {
                            if error.kind() != std::io::ErrorKind::NotFound {
                                warn!(path = %entry.path().display(), %error, "Failed to remove completed pack test candidate");
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(install_id, %error, "Failed to inspect pack test candidate")
                    }
                }
            }
        }
    });

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

        let authz_mq_url = mq_config.url.clone();
        tokio::spawn(async move {
            authz_metadata_invalidation_loop(authz_mq_url).await;
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
    let log_stream_wakeups = state.log_stream_wakeups.clone();
    let listener_db = database.pool().clone();
    let _postgres_listener =
        postgres_listener::spawn_postgres_listener(listener_db, broadcast_tx, log_stream_wakeups);

    info!("PostgreSQL notification listener started");

    let timeout_db = database.pool().clone();
    tokio::spawn(async move {
        inquiry_timeout::start_inquiry_timeout_monitor(timeout_db).await;
    });
    info!("Inquiry timeout monitor started");

    // Create and start server
    let server = Server::new(state.clone());

    info!("Attune API Service is ready");

    let shutdown_streams = state.execution_log_streams.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Received shutdown signal");
        shutdown_streams.begin_shutdown();
    });

    if let Err(e) = server.run().await {
        tracing::error!("Server error: {}", e);
        return Err(e);
    }

    info!("Shutting down Attune API Service");

    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler");
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.expect("failed to install Ctrl-C handler"),
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl-C handler");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_upgrade_failure_keeps_missing_bytes_context() {
        let report = attune_api::pack_release_upgrade::PackReleaseUpgradeReport {
            upgraded: vec![],
            failures: vec![
                attune_api::pack_release_upgrade::PackReleaseUpgradeFailure {
                    pack_ref: "missing-pack".to_string(),
                    error: "restore the exact installed bytes and force-register it".to_string(),
                },
            ],
        };

        let error = report_legacy_pack_upgrade(&report, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing-pack"));
        assert!(error.contains("restore the exact installed bytes"));
        assert!(error.contains("force-register"));
    }

    #[test]
    fn abandoned_pack_staging_requires_an_anonymous_staging_name_and_full_ttl() {
        assert!(is_abandoned_pack_staging(
            ".demo.123.staging",
            Some(ABANDONED_PACK_STAGING_TTL)
        ));
        assert!(!is_abandoned_pack_staging(
            ".demo.123.staging",
            ABANDONED_PACK_STAGING_TTL.checked_sub(std::time::Duration::from_secs(1))
        ));
        assert!(!is_abandoned_pack_staging(
            "demo.staging",
            Some(ABANDONED_PACK_STAGING_TTL)
        ));
        assert!(!is_abandoned_pack_staging(
            ".pack-test-42",
            Some(ABANDONED_PACK_STAGING_TTL)
        ));
        assert!(!is_abandoned_pack_staging(".demo.123.staging", None));
    }

    #[test]
    fn parses_one_shot_pack_release_upgrade() {
        let args = Args::try_parse_from([
            "attune-api",
            "--config",
            "/etc/attune/attune.yaml",
            "--upgrade-pack-releases",
        ])
        .unwrap();

        assert!(args.upgrade_pack_releases);
        assert!(!args.migrate);
    }

    #[test]
    fn one_shot_pack_release_upgrade_can_apply_migrations_first() {
        let args =
            Args::try_parse_from(["attune-api", "--migrate", "--upgrade-pack-releases"]).unwrap();

        assert!(args.migrate);
        assert!(args.upgrade_pack_releases);
    }

    #[test]
    fn one_shot_legacy_pack_upgrade_fails_when_a_pack_fails() {
        let report = attune_api::pack_release_upgrade::PackReleaseUpgradeReport {
            upgraded: vec!["working".to_string()],
            failures: vec![
                attune_api::pack_release_upgrade::PackReleaseUpgradeFailure {
                    pack_ref: "broken".to_string(),
                    error: "missing directory".to_string(),
                },
            ],
        };

        let error = report_legacy_pack_upgrade(&report, true).unwrap_err();
        assert_eq!(
            error.to_string(),
            "failed to upgrade 1 legacy pack(s): broken: missing directory"
        );
    }

    #[test]
    fn startup_legacy_pack_upgrade_leaves_repair_api_available() {
        let report = attune_api::pack_release_upgrade::PackReleaseUpgradeReport {
            upgraded: vec![],
            failures: vec![
                attune_api::pack_release_upgrade::PackReleaseUpgradeFailure {
                    pack_ref: "broken".to_string(),
                    error: "missing directory".to_string(),
                },
            ],
        };

        assert!(report_legacy_pack_upgrade(&report, false).is_ok());
    }
}
