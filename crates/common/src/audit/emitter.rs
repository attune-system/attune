//! `AuditEmitter` — clone-able non-blocking handle used by services to record
//! audit events.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{mpsc::Sender, oneshot};
use tracing::warn;

#[derive(Debug, Default)]
pub struct AuditStats {
    pub emitted: AtomicU64,
    pub persisted: AtomicU64,
    pub lost: AtomicU64,
    pub retries: AtomicU64,
    pub insert_failures: AtomicU64,
    pub(crate) shutdown: tokio_util::sync::CancellationToken,
}

use super::PendingAuditEvent;

/// Each admitted event accounts its own terminal outcome, including task abort
/// and queue destruction. Aggregate snapshots cannot safely account concurrent
/// senders during shutdown.
pub(crate) struct TrackedAuditEvent {
    pub event: PendingAuditEvent,
    stats: Arc<AuditStats>,
    confirmed: bool,
}
impl TrackedAuditEvent {
    pub fn confirm(&mut self) {
        self.confirmed = true;
        self.stats.persisted.fetch_add(1, Ordering::Relaxed);
    }
}
impl std::ops::Deref for TrackedAuditEvent {
    type Target = PendingAuditEvent;
    fn deref(&self) -> &PendingAuditEvent {
        &self.event
    }
}
impl Drop for TrackedAuditEvent {
    fn drop(&mut self) {
        if !self.confirmed {
            self.stats.lost.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub(crate) enum AuditWriterMessage {
    Event(Box<TrackedAuditEvent>),
    Flush(oneshot::Sender<bool>),
    Shutdown,
}

/// Clone-able handle. Sending is non-blocking and lock-free.
///
/// If the writer task has been dropped (e.g. during shutdown) the send is
/// silently logged and discarded — audit emission must never break the
/// request path.
#[derive(Debug, Clone)]
pub struct AuditEmitter {
    tx: Option<Sender<AuditWriterMessage>>,
    pub stats: Arc<AuditStats>,
}

impl AuditEmitter {
    /// Construct an emitter that pushes onto the given channel.
    pub(crate) fn new(tx: Sender<AuditWriterMessage>) -> Self {
        Self {
            tx: Some(tx),
            stats: Arc::default(),
        }
    }

    /// Construct a no-op emitter. Useful in tests, or where audit logging is
    /// disabled by configuration.
    pub fn noop() -> Self {
        Self {
            tx: None,
            stats: Arc::default(),
        }
    }

    /// Returns true if this emitter is configured to actually send events.
    pub fn is_active(&self) -> bool {
        self.tx.is_some()
    }

    /// Emit an event. Returns immediately. Failures are logged at WARN level
    /// and dropped.
    pub fn emit(&self, event: PendingAuditEvent) {
        let Some(tx) = &self.tx else {
            return;
        };
        self.stats.emitted.fetch_add(1, Ordering::Relaxed);
        if tx
            .try_send(AuditWriterMessage::Event(Box::new(TrackedAuditEvent {
                event,
                stats: self.stats.clone(),
                confirmed: false,
            })))
            .is_err()
        {
            let lost = self.stats.lost.load(Ordering::Relaxed);
            if lost % 1024 == 1 {
                warn!(lost, "audit emitter: queue unavailable, audit events lost");
            }
        }
    }

    /// Wait until every event queued before this call has been written.
    ///
    /// Returns false when no writer is configured or the writer has stopped.
    pub async fn flush(&self) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let (ack_tx, ack_rx) = oneshot::channel();
            if tx.send(AuditWriterMessage::Flush(ack_tx)).await.is_err() {
                return false;
            }
            ack_rx.await.unwrap_or(false) && self.stats.lost.load(Ordering::Relaxed) == 0
        })
        .await
        .unwrap_or(false)
    }

    pub(crate) async fn request_shutdown(&self) {
        self.shutdown_writer();
    }

    pub(crate) fn shutdown_writer(&self) {
        self.stats.shutdown.cancel();
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(AuditWriterMessage::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditCategory, AuditEventBuilder, AuditOutcome};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn emit_via_channel() {
        let (tx, mut rx) = mpsc::channel(16);
        let emitter = AuditEmitter::new(tx);
        emitter.emit(
            AuditEventBuilder::new(AuditCategory::Api, "api.request", AuditOutcome::Success)
                .build(),
        );
        let received = rx.recv().await.expect("event received");
        let AuditWriterMessage::Event(received) = received else {
            panic!("expected audit event");
        };
        assert_eq!(received.event_type, "api.request");
    }

    #[tokio::test]
    async fn noop_emitter_does_nothing() {
        let emitter = AuditEmitter::noop();
        assert!(!emitter.is_active());
        emitter.emit(
            AuditEventBuilder::new(AuditCategory::Api, "api.request", AuditOutcome::Success)
                .build(),
        );
    }

    #[tokio::test]
    async fn dropped_receiver_is_logged_not_panicked() {
        let (tx, rx) = mpsc::channel(16);
        drop(rx);
        let emitter = AuditEmitter::new(tx);
        emitter.emit(
            AuditEventBuilder::new(AuditCategory::Api, "api.request", AuditOutcome::Success)
                .build(),
        );
    }
    #[tokio::test]
    async fn bounded_overload_accounts_each_rejected_event() {
        let (tx, mut rx) = mpsc::channel(1);
        let emitter = AuditEmitter::new(tx);
        for _ in 0..3 {
            emitter.emit(
                AuditEventBuilder::new(AuditCategory::Api, "probe", AuditOutcome::Success).build(),
            );
        }
        assert_eq!(emitter.stats.emitted.load(Ordering::Relaxed), 3);
        assert_eq!(emitter.stats.lost.load(Ordering::Relaxed), 2);
        let Some(AuditWriterMessage::Event(mut event)) = rx.recv().await else {
            panic!("event expected")
        };
        event.confirm();
        drop(event);
        let stats = emitter.stats.clone();
        let task = tokio::spawn(async move {
            let AuditWriterMessage::Flush(ack) = rx.recv().await.unwrap() else {
                panic!("flush expected")
            };
            ack.send(true).unwrap();
        });
        assert!(
            !emitter.flush().await,
            "overload cannot be reported as persisted"
        );
        task.await.unwrap();
        assert_eq!(stats.lost.load(Ordering::Relaxed), 2);
    }
    #[tokio::test]
    async fn aborted_writer_accounts_buffered_and_queued_events_once() {
        let (tx, mut rx) = mpsc::channel(4);
        let emitter = AuditEmitter::new(tx);
        for _ in 0..2 {
            emitter.emit(
                AuditEventBuilder::new(AuditCategory::Api, "probe.abort", AuditOutcome::Success)
                    .build(),
            );
        }
        let (ready, observed) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _inflight = rx.recv().await.unwrap();
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        observed.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(emitter.stats.lost.load(Ordering::Relaxed), 2);
        assert_eq!(emitter.stats.persisted.load(Ordering::Relaxed), 0);
        emitter.emit(
            AuditEventBuilder::new(AuditCategory::Api, "probe.closed", AuditOutcome::Success)
                .build(),
        );
        assert_eq!(emitter.stats.lost.load(Ordering::Relaxed), 3);
        assert_eq!(emitter.stats.emitted.load(Ordering::Relaxed), 3);
    }
}
