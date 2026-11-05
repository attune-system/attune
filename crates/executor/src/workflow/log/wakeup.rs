//! LISTEN is a hint; no delivery or claim ownership lives on this connection.
use std::time::Duration;

use attune_common::{
    db::ConnectionIdentity, repositories::workflow_log_outbox::WORKFLOW_LOG_WAKEUP_CHANNEL,
};
use futures::{future::BoxFuture, FutureExt};
use sqlx::{
    postgres::{PgListener, PgPoolOptions},
    PgPool,
};
use tokio::{sync::broadcast, time::Instant};
use tracing::warn;
use uuid::Uuid;

pub(super) const PENDING_DELAY: Duration = Duration::from_millis(100);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) struct OutboxWakeups {
    pool: PgPool,
    pub(super) listener: Option<PgListener>,
    connecting: Option<BoxFuture<'static, sqlx::Result<PgListener>>>,
    retry_at: Instant,
    empty_delay: Duration,
}

impl OutboxWakeups {
    pub(super) fn new(service_pool: &PgPool, owner: Uuid) -> Self {
        let identity =
            ConnectionIdentity::new("executor", owner.to_string(), "workflow-log-wakeup");
        let options = (*service_pool.connect_options())
            .clone()
            .application_name(&identity.application_name());
        // One dedicated backend, independent of the work pool's capacity. No
        // spawned task: connection attempts advance inside the dispatcher future.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .max_lifetime(None)
            .idle_timeout(None)
            .acquire_timeout(CONNECT_TIMEOUT)
            .connect_lazy_with(options);
        Self {
            pool,
            listener: None,
            connecting: None,
            retry_at: Instant::now(),
            // Stagger empty fallback scans across replicas; never exceed 5 s.
            empty_delay: Duration::from_millis(4000 + u64::from(owner.as_bytes()[0]) * 1000 / 255),
        }
    }

    /// false means shutdown. A new subscription/reconnect always causes an
    /// immediate scan, closing the initial-scan / LISTEN registration race.
    pub(super) async fn wait(
        &mut self,
        pending: bool,
        shutdown: &mut broadcast::Receiver<()>,
    ) -> bool {
        if let Some(listener) = self.listener.as_mut() {
            let delay = if pending {
                PENDING_DELAY
            } else {
                self.empty_delay
            };
            let received = tokio::select! {
                _ = shutdown.recv() => return false,
                _ = tokio::time::sleep(delay) => return true,
                result = listener.try_recv() => result,
            };
            match received {
                Ok(Some(_)) => self.drain_ready(),
                result => {
                    if let Err(error) = result {
                        warn!(%error, "Workflow log wakeup listener disconnected; polling until reconnected");
                    }
                    self.listener = None;
                    self.retry_at = Instant::now() + RECONNECT_DELAY;
                }
            }
            return true;
        }

        if self.connecting.is_none() && Instant::now() >= self.retry_at {
            let pool = self.pool.clone();
            self.connecting = Some(Box::pin(async move {
                tokio::time::timeout(CONNECT_TIMEOUT, async {
                    let mut listener = PgListener::connect_with(&pool).await?;
                    // Match the notifier's cancellation-safe dedicated sockets.
                    listener.abort_on_drop();
                    listener.eager_reconnect(false);
                    listener.listen_all([WORKFLOW_LOG_WAKEUP_CHANNEL]).await?;
                    Ok(listener)
                })
                .await
                .map_err(|_| sqlx::Error::PoolTimedOut)?
            }));
        }
        let result = tokio::select! {
            _ = shutdown.recv() => return false,
            // Never lengthen work detection while LISTEN is unavailable.
            _ = tokio::time::sleep(PENDING_DELAY) => return true,
            result = async {
                match self.connecting.as_mut() {
                    Some(attempt) => attempt.await,
                    None => std::future::pending().await,
                }
            } => result,
        };
        self.connecting = None;
        match result {
            Ok(listener) => self.listener = Some(listener),
            Err(error) => {
                warn!(%error, "Workflow log wakeup subscription failed; polling until reconnected");
                self.retry_at = Instant::now() + RECONNECT_DELAY;
            }
        }
        true
    }

    /// Consume a bounded batch without waiting, including while delivery stays
    /// busy. Do not leave an idle LISTEN backend accumulating a backlog forever.
    pub(super) fn drain_ready(&mut self) {
        if self.listener.is_none() && self.connecting.is_some() {
            // Work resumed while LISTEN was being established. Do not leave
            // a partly registered, unread socket suspended through a backlog.
            self.connecting = None;
            self.retry_at = Instant::now() + RECONNECT_DELAY;
        }
        for _ in 0..64 {
            let Some(listener) = self.listener.as_mut() else {
                return;
            };
            match listener.try_recv().now_or_never() {
                Some(Ok(Some(_))) => {}
                Some(result) => {
                    if let Err(error) = result {
                        warn!(%error, "Workflow log wakeup listener disconnected during delivery");
                    }
                    self.listener = None;
                    self.retry_at = Instant::now() + RECONNECT_DELAY;
                    return;
                }
                None => return,
            }
        }
    }

    /// A dispatcher unable to process work must not keep an unread LISTEN
    /// backend indefinitely. Resume subscription when reconciliation can run.
    pub(super) fn disconnect(&mut self) {
        self.connecting = None;
        self.listener = None;
        self.retry_at = Instant::now() + RECONNECT_DELAY;
    }

    pub(super) async fn close(mut self) {
        self.connecting = None;
        self.listener = None;
        self.pool.close().await;
    }
}
