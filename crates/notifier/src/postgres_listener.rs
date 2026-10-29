//! PostgreSQL LISTEN/NOTIFY integration for real-time notifications

use anyhow::{Context, Result};
use sqlx::postgres::PgListener;
use sqlx::{Acquire, Connection};
use std::future::Future;
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

const IDLE_INTERVAL: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// Keep the receive/recovery loop testable without a database or real-time waits.
#[async_trait::async_trait]
trait ListenConnection: Send {
    type Notification;

    async fn recv(&mut self) -> Result<Self::Notification>;
    async fn ping(&mut self) -> Result<()>;
}

#[async_trait::async_trait]
impl ListenConnection for PgListener {
    type Notification = sqlx::postgres::PgNotification;

    async fn recv(&mut self) -> Result<Self::Notification> {
        // Return control to our bounded reconnect loop on detected I/O failure.
        self.try_recv()
            .await?
            .context("PostgreSQL LISTEN connection lost")
    }

    async fn ping(&mut self) -> Result<()> {
        Ok(self.acquire().await?.ping().await?)
    }
}

async fn run_listener<C, F, Fut>(
    mut connect: F,
    mut on_notification: impl FnMut(C::Notification),
) -> Result<()>
where
    C: ListenConnection,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<C>>,
{
    let mut listener = tokio::time::timeout(CONNECT_TIMEOUT, connect())
        .await
        .context("PostgreSQL listener connection/subscription timed out")??;

    loop {
        trace!("Calling listener.recv() — waiting for next notification");
        let received = match tokio::time::timeout(IDLE_INTERVAL, listener.recv()).await {
            Ok(result) => result.map(Some),
            // A pending receive cannot detect a silently broken socket. Probe the
            // dedicated LISTEN connection, not the service's role lookup pool.
            Err(_) => tokio::time::timeout(PROBE_TIMEOUT, listener.ping())
                .await
                .context("PostgreSQL LISTEN connection probe timed out")
                .and_then(|result| result)
                .map(|()| None),
        };
        match received {
            Ok(Some(notification)) => on_notification(notification),
            Ok(None) => info!("PostgreSQL listener heartbeat: LISTEN connection probe succeeded"),
            Err(e) => {
                warn!("PostgreSQL listener connection failed: {:#}", e);
                // Never return to recv on this connection, even if reconnect fails.
                drop(listener);
                tokio::time::sleep(Duration::from_secs(1)).await;
                loop {
                    warn!("Attempting to reconnect PostgreSQL listener...");
                    let reconnected = tokio::time::timeout(CONNECT_TIMEOUT, connect())
                        .await
                        .context("PostgreSQL listener connection/subscription timed out")
                        .and_then(|result| result);
                    match reconnected {
                        Ok(new_listener) => {
                            listener = new_listener;
                            info!("PostgreSQL listener reconnected successfully");
                            break;
                        }
                        Err(e) => {
                            error!("Failed to reconnect PostgreSQL listener: {:#}", e);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
    }
}

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

        run_listener(
            || self.create_listener(),
            |notification| {
                let channel = notification.channel();
                let payload = notification.payload();
                debug!(
                    "Received PostgreSQL notification: channel={}, payload_len={}",
                    channel,
                    payload.len()
                );
                if let Err(e) = self.process_notification(channel, payload) {
                    error!(
                        "Failed to process notification from channel '{}': {}",
                        channel, e
                    );
                }
            },
        )
        .await
    }

    /// Create a fresh [`PgListener`] subscribed to all notification channels.
    async fn create_listener(&self) -> Result<PgListener> {
        info!(
            "Connecting PostgreSQL LISTEN connection to {}",
            crate::mask_connection_url(&self.database_url)
        );

        let mut listener = PgListener::connect(&self.database_url)
            .await
            .context("Failed to connect PostgreSQL listener")?;

        // SQLx normally spawns an unbounded UNLISTEN during Drop. Close this
        // dedicated socket directly on failure or cancellation, even during LISTEN.
        listener.abort_on_drop();
        listener.eager_reconnect(false);

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
#[path = "postgres_listener_connection_tests.rs"]
mod connection_tests;

#[cfg(test)]
mod tests {
    use super::*;

    mod liveness {
        use super::*;
        use std::{
            collections::BTreeMap,
            sync::{Arc, Mutex},
        };

        #[derive(Clone, Copy)]
        enum Outcome {
            Healthy,
            Failed,
            Stalled,
        }

        #[derive(Default, Debug)]
        struct Calls {
            connects: usize,
            connect_times: Vec<Duration>,
            receives: BTreeMap<usize, usize>,
            probes: BTreeMap<usize, usize>,
            dropped: Vec<usize>,
        }

        struct FakeConnection {
            id: usize,
            calls: Arc<Mutex<Calls>>,
            probe: Outcome,
            receive_error: bool,
            notification: Option<i64>,
        }

        impl Drop for FakeConnection {
            fn drop(&mut self) {
                self.calls.lock().unwrap().dropped.push(self.id);
            }
        }

        #[async_trait::async_trait]
        impl ListenConnection for FakeConnection {
            type Notification = i64;

            async fn recv(&mut self) -> Result<i64> {
                *self
                    .calls
                    .lock()
                    .unwrap()
                    .receives
                    .entry(self.id)
                    .or_default() += 1;
                if self.receive_error {
                    anyhow::bail!("explicit receive failure");
                }
                if let Some(notification) = self.notification.take() {
                    return Ok(notification);
                }
                std::future::pending().await
            }

            async fn ping(&mut self) -> Result<()> {
                *self
                    .calls
                    .lock()
                    .unwrap()
                    .probes
                    .entry(self.id)
                    .or_default() += 1;
                match self.probe {
                    Outcome::Healthy => Ok(()),
                    Outcome::Failed => anyhow::bail!("broken LISTEN connection"),
                    Outcome::Stalled => std::future::pending().await,
                }
            }
        }

        // Exercise the production timeout and reconnect loop, including parsing and
        // delivery to the same broadcast receiver after recovery. Only I/O is fake.
        async fn scenario(
            probe: Outcome,
            receive_error: bool,
            connect_outcomes: &[Outcome],
        ) -> (Calls, Vec<i64>) {
            let calls = Arc::new(Mutex::new(Calls::default()));
            let (tx, mut rx) = broadcast::channel(4);
            let notifier = PostgresListener::new("unused".into(), tx).await.unwrap();
            let start = tokio::time::Instant::now();
            let mut outcomes = connect_outcomes.iter().copied();
            let connect = || {
                let outcome = outcomes.next().expect("unexpected connection attempt");
                let calls = calls.clone();
                let id = {
                    let mut calls = calls.lock().unwrap();
                    calls.connects += 1;
                    calls.connect_times.push(start.elapsed());
                    assert!(
                        calls.connects == 1 || calls.dropped.contains(&1),
                        "discard the failed connection before attempting reconnect"
                    );
                    calls.connects
                };
                async move {
                    match outcome {
                        Outcome::Failed => anyhow::bail!("reconnect failed"),
                        Outcome::Stalled => std::future::pending().await,
                        Outcome::Healthy => Ok(FakeConnection {
                            id,
                            calls,
                            probe: if id == 1 { probe } else { Outcome::Healthy },
                            receive_error: id == 1 && receive_error,
                            notification: (id > 1).then_some(123),
                        }),
                    }
                }
            };
            let result = tokio::time::timeout(
                Duration::from_secs(181),
                run_listener(connect, |id| {
                    notifier
                        .process_notification(
                            "execution_status_changed",
                            &format!(r#"{{"entity_type":"execution","entity_id":{id}}}"#),
                        )
                        .unwrap();
                }),
            )
            .await;
            assert!(
                result.is_err(),
                "listener loop must keep running: {result:?}"
            );
            let mut delivered = Vec::new();
            while let Ok(notification) = rx.try_recv() {
                delivered.push(notification.entity_id);
            }
            let calls = Arc::try_unwrap(calls).unwrap().into_inner().unwrap();
            (calls, delivered)
        }

        #[tokio::test(start_paused = true)]
        async fn healthy_idle_connection_is_probed_without_reconnecting() {
            let (calls, delivered) = scenario(Outcome::Healthy, false, &[Outcome::Healthy]).await;
            assert_eq!(
                calls.probes.get(&1),
                Some(&3),
                "idle timeouts must probe the LISTEN connection"
            );
            assert_eq!(calls.connects, 1);
            assert!(delivered.is_empty());
        }

        #[tokio::test(start_paused = true)]
        async fn failed_idle_probe_reconnects_and_resumes_delivery() {
            let (calls, delivered) = scenario(
                Outcome::Failed,
                false,
                &[Outcome::Healthy, Outcome::Healthy],
            )
            .await;
            assert_eq!(calls.connects, 2, "a failed idle probe must reconnect");
            assert_eq!(calls.connect_times[1], Duration::from_secs(61));
            assert_eq!(calls.receives.get(&1), Some(&1));
            assert_eq!(calls.dropped, vec![1, 2]);
            assert_eq!(delivered, vec![123]);
        }

        #[tokio::test(start_paused = true)]
        async fn stalled_idle_probe_reconnects_and_resumes_delivery() {
            let (calls, delivered) = scenario(
                Outcome::Stalled,
                false,
                &[Outcome::Healthy, Outcome::Healthy],
            )
            .await;
            assert_eq!(
                calls.connects, 2,
                "a stalled idle probe must have a deadline"
            );
            assert_eq!(calls.connect_times[1], Duration::from_secs(71));
            assert_eq!(calls.receives.get(&1), Some(&1));
            assert_eq!(delivered, vec![123]);
        }

        #[tokio::test(start_paused = true)]
        async fn reconnect_retries_errors_and_timeouts_without_reusing_dead_connection() {
            let (calls, delivered) = scenario(
                Outcome::Failed,
                false,
                &[
                    Outcome::Healthy,
                    Outcome::Failed,
                    Outcome::Stalled,
                    Outcome::Healthy,
                ],
            )
            .await;
            assert_eq!(
                calls.connects, 4,
                "retry failed and stalled reconnects independently of recv"
            );
            assert_eq!(
                calls.connect_times,
                [0, 61, 66, 81].map(Duration::from_secs)
            );
            assert_eq!(calls.receives.get(&1), Some(&1));
            assert_eq!(calls.dropped, vec![1, 4]);
            assert_eq!(delivered, vec![123]);
        }

        #[tokio::test(start_paused = true)]
        async fn explicit_receive_error_still_reconnects() {
            let (calls, delivered) = scenario(
                Outcome::Healthy,
                true,
                &[Outcome::Healthy, Outcome::Healthy],
            )
            .await;
            assert_eq!(calls.connects, 2);
            assert_eq!(calls.receives.get(&1), Some(&1));
            assert_eq!(delivered, vec![123]);
        }

        #[tokio::test(start_paused = true)]
        async fn initial_connection_has_a_deadline() {
            let result = tokio::time::timeout(
                Duration::from_secs(11),
                run_listener(std::future::pending::<Result<FakeConnection>>, |_| {}),
            )
            .await;
            assert!(
                matches!(result, Ok(Err(_))),
                "initial connection must fail within its deadline: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn listener_recreation_keeps_receivers_open_but_cannot_replay_artifact_changes(
    ) -> Result<()> {
        use attune_common::{
            config::Config,
            models::{
                ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType,
                RetentionPolicyType,
            },
            repositories::{
                artifact::{ArtifactRepository, CreateArtifactInput},
                artifact_read_diagnostics::ArtifactReadDiagnostics,
                Create, FindById,
            },
            test_database::TestDatabase,
        };
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let db = TestDatabase::create(&config.database)
            .await?
            .with_cleanup_on_drop();
        let (tx, mut receiver) = broadcast::channel(4);
        let listener = PostgresListener::new(db.database_url().to_string(), tx).await?;
        let mut pg = listener.create_listener().await?;
        // A healthy idle connection survives a cancelled receive and a real
        // protocol round-trip on the subscribed connection.
        assert!(tokio::time::timeout(Duration::from_millis(1), pg.recv())
            .await
            .is_err());
        tokio::time::timeout(PROBE_TIMEOUT, ListenConnection::ping(&mut pg)).await??;
        let create = |reference: &str| CreateArtifactInput {
            r#ref: reference.into(),
            scope: OwnerType::Action,
            owner: "listener.fixture".into(),
            r#type: ArtifactType::Progress,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 4,
            name: None,
            description: None,
            content_type: None,
            data: Some(serde_json::json!({})),
        };
        let first = ArtifactRepository::create(db.pool(), create("listener.first")).await?;
        // Notifications encountered by the ping must remain buffered for recv.
        tokio::time::timeout(PROBE_TIMEOUT, ListenConnection::ping(&mut pg)).await??;
        let notification = tokio::time::timeout(Duration::from_secs(10), pg.recv()).await??;
        listener.process_notification(notification.channel(), notification.payload())?;
        assert_eq!(receiver.recv().await?.entity_id, first.id);

        // Await UNLISTEN before the gap, rather than racing a connection Drop.
        // Recreating the listener never closes the existing broadcast receiver.
        pg.unlisten_all().await?;
        drop(pg);
        let missed = ArtifactRepository::create(db.pool(), create("listener.missed")).await?;
        assert!(!receiver.is_closed());
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let mut pg = listener.create_listener().await?;
        let latest = ArtifactRepository::create(db.pool(), create("listener.latest")).await?;
        let notification = tokio::time::timeout(Duration::from_secs(10), pg.recv()).await??;
        listener.process_notification(notification.channel(), notification.payload())?;
        assert_eq!(receiver.recv().await?.entity_id, latest.id);
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(
            ArtifactRepository::find_by_id(db.pool(), missed.id)
                .await?
                .is_some(),
            "reconciliation can still find the missed durable change"
        );
        pg.unlisten_all().await?;
        drop(pg);
        drop(listener);
        drop(receiver);
        // Observe server-side closure before asking the fixture to remove its database.
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut retry = tokio::time::interval(Duration::from_millis(10));
            loop {
                retry.tick().await;
                let (_, sessions) = ArtifactReadDiagnostics::wait_sample(db.pool()).await?;
                if sessions <= i64::from(db.pool().size()) {
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        db.cleanup().await?;
        Ok(())
    }

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
    fn test_process_notification_compact_deferred_payload_routes() {
        // A compact/deferred fallback payload (emitted when the rich payload
        // exceeds the NOTIFY size limit) drops the rich fields but retains the
        // routing core; the notifier still produces a valid Notification and
        // relies on a downstream DB visibility check.
        let (tx, mut rx) = broadcast::channel(10);
        let listener = PostgresListener {
            database_url: "postgresql://test".to_string(),
            notification_tx: tx,
        };

        let payload = serde_json::json!({
            "entity_type": "enforcement",
            "entity_id": 789,
            "id": 789,
            "rule_ref": "core.rule",
            "status": "active",
            "auth_mode": "deferred"
        });

        let result =
            listener.process_notification("enforcement_status_changed", &payload.to_string());
        assert!(result.is_ok());

        let notification = rx.try_recv().unwrap();
        assert_eq!(notification.entity_type, "enforcement");
        assert_eq!(notification.entity_id, 789);
        assert_eq!(notification.payload.get("auth_mode").unwrap(), "deferred");
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
