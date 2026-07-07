//! Notifier Service - Real-time notification orchestration

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, error, info};

use attune_common::config::Config;

use crate::postgres_listener::PostgresListener;
use crate::subscriber_manager::SubscriberManager;
use crate::websocket_server::WebSocketServer;

/// Notification message that can be broadcast to subscribers
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Notification {
    /// Type of notification (e.g., "execution_status_changed", "inquiry_created")
    pub notification_type: String,

    /// Entity type (e.g., "execution", "inquiry", "enforcement")
    pub entity_type: String,

    /// Entity ID
    pub entity_id: i64,

    /// Optional user/identity ID that should receive this notification
    pub user_id: Option<i64>,

    /// Notification payload (varies by type)
    pub payload: serde_json::Value,

    /// Timestamp when notification was created
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Main notifier service that coordinates all components
pub struct NotifierService {
    config: Config,
    postgres_listener: Arc<PostgresListener>,
    subscriber_manager: Arc<SubscriberManager>,
    websocket_server: WebSocketServer,
    shutdown_tx: broadcast::Sender<()>,
    db_pool: sqlx::PgPool,
}

impl NotifierService {
    /// Create a new notifier service
    pub async fn new(config: Config) -> Result<Self> {
        info!("Initializing Notifier Service");

        // Create shutdown broadcast channel
        let (shutdown_tx, _) = broadcast::channel(16);

        // Create notification broadcast channel
        let (notification_tx, _) = broadcast::channel(1000);

        // Create subscriber manager
        let subscriber_manager = Arc::new(SubscriberManager::new());

        // Create PostgreSQL listener
        let postgres_listener = Arc::new(
            PostgresListener::new(config.database.url.clone(), notification_tx.clone()).await?,
        );

        // Create a small connection pool for ad-hoc queries (role lookups on
        // WebSocket connect). The notifier is not a heavy DB consumer — its
        // primary DB workload is the dedicated LISTEN/NOTIFY connection in
        // `PostgresListener`. A 4-connection cap is plenty for occasional
        // role lookups while keeping the pool lightweight.
        let db_pool = PgPoolOptions::new()
            .max_connections(4)
            .min_connections(0)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(60))
            .connect(&config.database.url)
            .await
            .context("Failed to create notifier database pool")?;

        // Create WebSocket server
        let websocket_server = WebSocketServer::new(
            config.clone(),
            notification_tx.clone(),
            subscriber_manager.clone(),
            shutdown_tx.clone(),
            db_pool.clone(),
        );

        Ok(Self {
            config,
            postgres_listener,
            subscriber_manager,
            websocket_server,
            shutdown_tx,
            db_pool,
        })
    }

    /// Start the notifier service
    pub async fn start(&self) -> Result<()> {
        info!("Starting Notifier Service components");

        // Start PostgreSQL listener
        let listener_handle = {
            let listener = self.postgres_listener.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            tokio::spawn(async move {
                tokio::select! {
                    result = listener.listen() => {
                        if let Err(e) = result {
                            error!("PostgreSQL listener error: {}", e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("PostgreSQL listener shutting down");
                    }
                }
            })
        };

        // Start notification broadcaster (forwards notifications to WebSocket clients)
        let broadcast_handle = {
            let subscriber_manager = self.subscriber_manager.clone();
            let db_pool = self.db_pool.clone();
            let mut notification_rx = self.websocket_server.notification_tx.subscribe();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        recv_result = notification_rx.recv() => {
                            match recv_result {
                                Ok(notification) => {
                                    debug!(
                                        "Broadcasting notification: type={}, entity_type={}, entity_id={}",
                                        notification.notification_type,
                                        notification.entity_type,
                                        notification.entity_id,
                                    );
                                    // Authorize once per identity, then fan out
                                    // to all that identity's connections.
                                    crate::websocket_server::dispatch_notification(
                                        &subscriber_manager,
                                        &db_pool,
                                        notification,
                                    )
                                    .await;
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    error!("Notification broadcaster lagged — dropped {} messages", n);
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    error!("Notification broadcast channel closed — broadcaster exiting");
                                    break;
                                }
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Notification broadcaster shutting down");
                            break;
                        }
                    }
                }
            })
        };

        // Start WebSocket server
        let server_handle = {
            let server = self.websocket_server.clone();
            tokio::spawn(async move {
                if let Err(e) = server.start().await {
                    error!("WebSocket server error: {}", e);
                }
            })
        };

        let notifier_config = self
            .config
            .notifier
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Notifier configuration not found in config"))?;

        info!(
            "Notifier Service started on {}:{}",
            notifier_config.host, notifier_config.port
        );

        // Wait for any task to complete (they shouldn't unless there's an error)
        tokio::select! {
            _ = listener_handle => {
                error!("PostgreSQL listener stopped unexpectedly");
            }
            _ = broadcast_handle => {
                error!("Notification broadcaster stopped unexpectedly");
            }
            _ = server_handle => {
                error!("WebSocket server stopped unexpectedly");
            }
        }

        Ok(())
    }

    /// Shutdown the notifier service gracefully
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down Notifier Service");

        // Send shutdown signal to all components
        let _ = self.shutdown_tx.send(());

        // Disconnect all WebSocket clients
        self.subscriber_manager.disconnect_all().await;

        info!("Notifier Service shutdown complete");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_serialization() {
        let notification = Notification {
            notification_type: "execution_status_changed".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 123,
            user_id: Some(456),
            payload: serde_json::json!({
                "status": "succeeded",
                "action": "core.echo"
            }),
            timestamp: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&notification).unwrap();
        let deserialized: Notification = serde_json::from_str(&json).unwrap();

        assert_eq!(
            notification.notification_type,
            deserialized.notification_type
        );
        assert_eq!(notification.entity_type, deserialized.entity_type);
        assert_eq!(notification.entity_id, deserialized.entity_id);
    }
}
