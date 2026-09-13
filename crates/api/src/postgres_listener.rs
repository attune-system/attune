//! PostgreSQL LISTEN/NOTIFY listener for SSE broadcasting

use sqlx::postgres::{PgListener, PgPool};
use tokio::sync::{broadcast, watch};
use tracing::{debug, error, info, warn};

use crate::log_stream_wakeups::LogStreamWakeups;

const LOG_STREAM_CHANNEL: &str = "log_stream_changed";
const RECONNECT_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(5);

const NOTIFICATION_CHANNELS: &[&str] = &[
    "execution_created",
    "execution_status_changed",
    "event_created",
    "enforcement_created",
    "enforcement_status_changed",
    "inquiry_created",
    "inquiry_responded",
    "inquiry_timeout",
    "workflow_execution_status_changed",
    "artifact_created",
    "artifact_updated",
    "work_queue_created",
    "work_queue_updated",
    "work_queue_item_created",
    "work_queue_item_updated",
    LOG_STREAM_CHANNEL,
];

#[derive(Debug, serde::Deserialize, PartialEq, Eq)]
struct LogStreamChange {
    stream_id: i64,
    artifact_version_id: i64,
    total_bytes: i64,
    sealed: bool,
}

/// Background PostgreSQL listener with an awaitable connection signal.
pub struct PostgresListener {
    task: tokio::task::JoinHandle<()>,
    ready: watch::Receiver<bool>,
}

impl PostgresListener {
    /// Wait until all notification channels have been registered successfully.
    pub async fn wait_until_ready(&mut self) -> anyhow::Result<()> {
        while !*self.ready.borrow() {
            self.ready
                .changed()
                .await
                .map_err(|_| anyhow::anyhow!("PostgreSQL notification listener stopped"))?;
        }
        Ok(())
    }

    /// Stop the background listener.
    pub fn abort(self) {
        self.task.abort();
    }
}

/// Start the resilient notification listener without blocking API startup.
pub fn spawn_postgres_listener(
    db: PgPool,
    broadcast_tx: broadcast::Sender<String>,
    log_stream_wakeups: LogStreamWakeups,
) -> PostgresListener {
    info!("Starting PostgreSQL notification listener for SSE broadcasting");
    let (ready_tx, ready) = watch::channel(false);
    let task = tokio::spawn(run_postgres_listener(
        db,
        broadcast_tx,
        log_stream_wakeups,
        ready_tx,
    ));
    PostgresListener { task, ready }
}

async fn run_postgres_listener(
    db: PgPool,
    broadcast_tx: broadcast::Sender<String>,
    log_stream_wakeups: LogStreamWakeups,
    ready: watch::Sender<bool>,
) {
    loop {
        let mut listener = match connect_listener(&db).await {
            Ok(listener) => listener,
            Err(error) => {
                error!(%error, "Failed to connect PostgreSQL notification listener; retrying");
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };
        let readers = log_stream_wakeups.wake_all();
        ready.send_replace(true);
        info!(readers, "PostgreSQL notification listener connected");

        loop {
            match listener.recv().await {
                Ok(notification) => {
                    route_notification(
                        notification.channel(),
                        notification.payload(),
                        &broadcast_tx,
                        &log_stream_wakeups,
                    );
                }
                Err(error) => {
                    ready.send_replace(false);
                    warn!(%error, "PostgreSQL notification listener disconnected; reconnecting");
                    break;
                }
            }
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn connect_listener(db: &PgPool) -> anyhow::Result<PgListener> {
    let mut listener = PgListener::connect_with(db).await?;
    listener
        .listen_all(NOTIFICATION_CHANNELS.iter().copied())
        .await?;
    Ok(listener)
}

fn route_notification(
    channel: &str,
    payload: &str,
    broadcast_tx: &broadcast::Sender<String>,
    log_stream_wakeups: &LogStreamWakeups,
) {
    debug!(channel, payload, "Received PostgreSQL notification");
    if channel == LOG_STREAM_CHANNEL {
        match serde_json::from_str::<LogStreamChange>(payload) {
            Ok(change) => {
                let interested = log_stream_wakeups.wake(change.stream_id);
                debug!(
                    stream_id = change.stream_id,
                    artifact_version_id = change.artifact_version_id,
                    total_bytes = change.total_bytes,
                    sealed = change.sealed,
                    interested,
                    "Routed log stream wakeup"
                );
            }
            Err(error) => warn!(%error, payload, "Ignoring invalid log stream notification"),
        }
        return;
    }

    match broadcast_tx.send(payload.to_string()) {
        Ok(receiver_count) => {
            debug!("Broadcasted notification to {} SSE clients", receiver_count);
        }
        Err(error) => {
            debug!("No active SSE clients to receive notification: {}", error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routes_compact_log_change_only_to_its_stream() {
        let (broadcast_tx, mut broadcast_rx) = broadcast::channel(1);
        let wakeups = LogStreamWakeups::default();
        let mut subscription = wakeups.subscribe(42);

        route_notification(
            LOG_STREAM_CHANNEL,
            r#"{"stream_id":42,"artifact_version_id":9,"total_bytes":128,"sealed":false}"#,
            &broadcast_tx,
            &wakeups,
        );

        assert!(subscription.wait(tokio::time::Duration::from_secs(1)).await);
        assert!(broadcast_rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_log_change_is_recovered_by_reader_reconciliation() {
        let (broadcast_tx, _) = broadcast::channel(1);
        let wakeups = LogStreamWakeups::default();
        let mut subscription = wakeups.subscribe(42);
        route_notification(LOG_STREAM_CHANNEL, "not-json", &broadcast_tx, &wakeups);

        let wait = tokio::spawn(async move {
            subscription
                .wait(tokio::time::Duration::from_secs(15))
                .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(tokio::time::Duration::from_secs(15)).await;
        assert!(!wait.await.unwrap());
    }
}
