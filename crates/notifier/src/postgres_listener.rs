//! PostgreSQL LISTEN/NOTIFY integration for real-time notifications

use anyhow::{Context, Result};
use sqlx::postgres::PgListener;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, error, info, trace, warn};

use crate::service::Notification;

/// Channels to listen on for PostgreSQL notifications
const NOTIFICATION_CHANNELS: &[&str] = &[
    "attune_notifications",
    "execution_status_changed",
    "execution_created",
    "inquiry_created",
    "inquiry_responded",
    "inquiry_timeout",
    "enforcement_created",
    "enforcement_status_changed",
    "event_created",
    "workflow_execution_status_changed",
    "artifact_created",
    "artifact_updated",
    "work_queue_created",
    "work_queue_updated",
    "work_queue_item_created",
    "work_queue_item_updated",
    "rule_lifecycle_changed",
];

/// PostgreSQL listener that receives NOTIFY events and broadcasts them
pub struct PostgresListener {
    database_url: String,
    notification_tx: broadcast::Sender<Notification>,
}

impl PostgresListener {
    /// Create a new PostgreSQL listener
    pub async fn new(
        database_url: String,
        notification_tx: broadcast::Sender<Notification>,
    ) -> Result<Self> {
        Ok(Self {
            database_url,
            notification_tx,
        })
    }

    /// Start listening for PostgreSQL notifications
    pub async fn listen(&self) -> Result<()> {
        info!(
            "Starting PostgreSQL LISTEN on channels: {:?}",
            NOTIFICATION_CHANNELS
        );

        // Create a dedicated listener connection
        let mut listener = self.create_listener().await?;

        info!("PostgreSQL listener ready — entering recv loop");

        // Periodic heartbeat so we can confirm the task is alive even when idle.
        let heartbeat_interval = Duration::from_secs(60);
        let mut next_heartbeat = tokio::time::Instant::now() + heartbeat_interval;

        // Process notifications in a loop
        loop {
            // Log a heartbeat if no notification has arrived for a while.
            let now = tokio::time::Instant::now();
            if now >= next_heartbeat {
                info!("PostgreSQL listener heartbeat — still waiting for notifications");
                next_heartbeat = now + heartbeat_interval;
            }

            trace!("Calling listener.recv() — waiting for next notification");

            // Use a timeout so the heartbeat fires even during long idle periods.
            match tokio::time::timeout(heartbeat_interval, listener.recv()).await {
                // Timed out waiting — loop back and log the heartbeat above.
                Err(_timeout) => {
                    trace!("listener.recv() timed out — re-entering loop");
                    continue;
                }
                Ok(recv_result) => match recv_result {
                    Ok(pg_notification) => {
                        let channel = pg_notification.channel();
                        let payload = pg_notification.payload();
                        debug!(
                            "Received PostgreSQL notification: channel={}, payload_len={}",
                            channel,
                            payload.len()
                        );
                        debug!("Notification payload: {}", payload);

                        // Parse and broadcast notification
                        if let Err(e) = self.process_notification(channel, payload) {
                            error!(
                                "Failed to process notification from channel '{}': {}",
                                channel, e
                            );
                        }
                    }
                    Err(e) => {
                        error!("Error receiving PostgreSQL notification: {}", e);

                        // Sleep briefly before retrying to avoid tight loop on persistent errors
                        tokio::time::sleep(Duration::from_secs(1)).await;

                        // Try to reconnect
                        warn!("Attempting to reconnect PostgreSQL listener...");
                        match self.create_listener().await {
                            Ok(new_listener) => {
                                listener = new_listener;
                                next_heartbeat = tokio::time::Instant::now() + heartbeat_interval;
                                info!("PostgreSQL listener reconnected successfully");
                            }
                            Err(e) => {
                                error!("Failed to reconnect PostgreSQL listener: {}", e);
                                tokio::time::sleep(Duration::from_secs(5)).await;
                            }
                        }
                    }
                }, // end Ok(recv_result)
            } // end timeout match
        }
    }

    /// Create a fresh [`PgListener`] subscribed to all notification channels.
    async fn create_listener(&self) -> Result<PgListener> {
        info!("Connecting PostgreSQL LISTEN connection to {}", {
            // Mask the password for logging
            let url = &self.database_url;
            if let Some(at) = url.rfind('@') {
                if let Some(colon) = url[..at].rfind(':') {
                    format!("{}:****{}", &url[..colon], &url[at..])
                } else {
                    url.clone()
                }
            } else {
                url.clone()
            }
        });

        let mut listener = PgListener::connect(&self.database_url)
            .await
            .context("Failed to connect PostgreSQL listener")?;

        info!("PostgreSQL LISTEN connection established — subscribing to channels");

        // Use listen_all for a single round-trip instead of N separate commands
        listener
            .listen_all(NOTIFICATION_CHANNELS.iter().copied())
            .await
            .context("Failed to LISTEN on notification channels")?;

        info!(
            "Subscribed to {} PostgreSQL channels: {:?}",
            NOTIFICATION_CHANNELS.len(),
            NOTIFICATION_CHANNELS
        );

        Ok(listener)
    }

    /// Process a PostgreSQL notification and broadcast it to WebSocket clients
    fn process_notification(&self, channel: &str, payload: &str) -> Result<()> {
        // Parse the JSON payload
        let payload_json: serde_json::Value = serde_json::from_str(payload)
            .context("Failed to parse notification payload as JSON")?;

        // Extract common fields
        let entity_type = payload_json
            .get("entity_type")
            .and_then(|v| v.as_str())
            .context("Missing 'entity_type' in notification payload")?
            .to_string();

        let entity_id = payload_json
            .get("entity_id")
            .and_then(|v| v.as_i64())
            .context("Missing 'entity_id' in notification payload")?;

        let user_id = payload_json.get("user_id").and_then(|v| v.as_i64());

        // Create notification
        let notification = Notification {
            notification_type: channel.to_string(),
            entity_type,
            entity_id,
            user_id,
            payload: payload_json,
            timestamp: chrono::Utc::now(),
        };

        // Broadcast to all subscribers (ignore errors if no receivers)
        match self.notification_tx.send(notification) {
            Ok(receiver_count) => {
                debug!(
                    "Broadcast notification to {} receivers: type={}, entity_id={}",
                    receiver_count, channel, entity_id
                );
            }
            Err(_) => {
                // No active receivers, this is fine
                debug!("No active receivers for notification: type={}", channel);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_channels_defined() {
        assert!(!NOTIFICATION_CHANNELS.is_empty());
        assert!(NOTIFICATION_CHANNELS.contains(&"execution_status_changed"));
        assert!(NOTIFICATION_CHANNELS.contains(&"enforcement_created"));
        assert!(NOTIFICATION_CHANNELS.contains(&"enforcement_status_changed"));
        assert!(NOTIFICATION_CHANNELS.contains(&"inquiry_created"));
        assert!(NOTIFICATION_CHANNELS.contains(&"artifact_created"));
        assert!(NOTIFICATION_CHANNELS.contains(&"artifact_updated"));
        assert!(NOTIFICATION_CHANNELS.contains(&"work_queue_created"));
        assert!(NOTIFICATION_CHANNELS.contains(&"work_queue_updated"));
        assert!(NOTIFICATION_CHANNELS.contains(&"work_queue_item_created"));
        assert!(NOTIFICATION_CHANNELS.contains(&"work_queue_item_updated"));
        assert!(NOTIFICATION_CHANNELS.contains(&"rule_lifecycle_changed"));
    }

    #[test]
    fn test_process_notification_valid_payload() {
        let (tx, mut rx) = broadcast::channel(10);
        let listener = PostgresListener {
            database_url: "postgresql://test".to_string(),
            notification_tx: tx,
        };

        let payload = serde_json::json!({
            "entity_type": "execution",
            "entity_id": 123,
            "user_id": 456,
            "status": "succeeded"
        });

        let result =
            listener.process_notification("execution_status_changed", &payload.to_string());

        assert!(result.is_ok());

        // Should receive the notification
        let notification = rx.try_recv().unwrap();
        assert_eq!(notification.notification_type, "execution_status_changed");
        assert_eq!(notification.entity_type, "execution");
        assert_eq!(notification.entity_id, 123);
        assert_eq!(notification.user_id, Some(456));
    }

    #[test]
    fn test_process_notification_missing_fields() {
        let (tx, _rx) = broadcast::channel(10);
        let listener = PostgresListener {
            database_url: "postgresql://test".to_string(),
            notification_tx: tx,
        };

        // Missing entity_id
        let payload = serde_json::json!({
            "entity_type": "execution"
        });

        let result =
            listener.process_notification("execution_status_changed", &payload.to_string());

        assert!(result.is_err());
    }

    #[test]
    fn test_process_notification_invalid_json() {
        let (tx, _rx) = broadcast::channel(10);
        let listener = PostgresListener {
            database_url: "postgresql://test".to_string(),
            notification_tx: tx,
        };

        let result = listener.process_notification("execution_status_changed", "not valid json");

        assert!(result.is_err());
    }
}
