//! Notifier Service - Real-time notification orchestration

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
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
    shutdown: CancellationToken,
    db_pool: sqlx::PgPool,
}

impl NotifierService {
    /// Create a new notifier service
    pub async fn new(config: Config) -> Result<Self> {
        info!("Initializing Notifier Service");

        let shutdown = CancellationToken::new();

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
            shutdown.clone(),
            db_pool.clone(),
        );

        Ok(Self {
            config,
            postgres_listener,
            subscriber_manager,
            websocket_server,
            shutdown,
            db_pool,
        })
    }

    /// Start the notifier service
    pub async fn start(&self) -> Result<()> {
        info!("Starting Notifier Service components");

        // Start PostgreSQL listener
        let mut tasks = JoinSet::new();

        {
            let listener = self.postgres_listener.clone();
            let shutdown = self.shutdown.clone();
            tasks.spawn(async move {
                let result = tokio::select! {
                    result = listener.listen() => result,
                    _ = shutdown.cancelled() => {
                        info!("PostgreSQL listener shutting down");
                        Ok(())
                    }
                };
                ("PostgreSQL listener", result)
            });
        }

        // Start notification broadcaster (forwards notifications to WebSocket clients)
        {
            let subscriber_manager = self.subscriber_manager.clone();
            let db_pool = self.db_pool.clone();
            let mut notification_rx = self.websocket_server.notification_tx.subscribe();
            let shutdown = self.shutdown.clone();
            tasks.spawn(async move {
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
                        _ = shutdown.cancelled() => {
                            info!("Notification broadcaster shutting down");
                            break;
                        }
                    }
                }
                ("Notification broadcaster", Ok(()))
            });
        }

        // Start WebSocket server
        {
            let server = self.websocket_server.clone();
            tasks.spawn(async move { ("WebSocket server", server.start().await) });
        }

        let notifier_config = self
            .config
            .notifier
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Notifier configuration not found in config"))?;

        info!(
            "Notifier Service started on {}:{}",
            notifier_config.host, notifier_config.port
        );

        let component_result = match tasks.join_next().await {
            Some(Ok((component, Ok(())))) if !self.shutdown.is_cancelled() => {
                Err(anyhow::anyhow!("{component} stopped unexpectedly"))
            }
            Some(Ok((_, result))) => result,
            Some(Err(error)) if error.is_cancelled() => Ok(()),
            Some(Err(error)) => Err(anyhow::anyhow!("Notifier component task failed: {error}")),
            None => Err(anyhow::anyhow!("Notifier started without component tasks")),
        };

        self.shutdown.cancel();
        self.subscriber_manager.disconnect_all().await;
        abort_and_join_tasks(&mut tasks).await;

        component_result
    }

    /// Shutdown the notifier service gracefully
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down Notifier Service");

        self.shutdown.cancel();

        // Disconnect all WebSocket clients
        self.subscriber_manager.disconnect_all().await;

        info!("Notifier Service shutdown complete");

        Ok(())
    }
}

async fn abort_and_join_tasks<T: 'static>(tasks: &mut JoinSet<T>) {
    tasks.abort_all();
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            if !error.is_cancelled() {
                error!("Notifier component task failed during shutdown: {}", error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[tokio::test]
    async fn aborted_component_tasks_are_joined() {
        struct Dropped(Arc<AtomicUsize>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicUsize::new(0));
        let mut tasks = JoinSet::new();
        for _ in 0..3 {
            let task_dropped = dropped.clone();
            tasks.spawn(async move {
                let _dropped = Dropped(task_dropped);
                std::future::pending::<()>().await;
            });
        }
        tokio::task::yield_now().await;

        abort_and_join_tasks(&mut tasks).await;

        assert_eq!(dropped.load(Ordering::SeqCst), 3);
        assert!(tasks.is_empty());
    }
}
