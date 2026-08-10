//! `AuditEmitter` — clone-able non-blocking handle used by services to record
//! audit events.

use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tracing::warn;

use super::PendingAuditEvent;

pub(crate) enum AuditWriterMessage {
    Event(Box<PendingAuditEvent>),
    Flush(oneshot::Sender<()>),
    Shutdown,
}

/// Clone-able handle. Sending is non-blocking and lock-free.
///
/// If the writer task has been dropped (e.g. during shutdown) the send is
/// silently logged and discarded — audit emission must never break the
/// request path.
#[derive(Debug, Clone)]
pub struct AuditEmitter {
    tx: Option<UnboundedSender<AuditWriterMessage>>,
}

impl AuditEmitter {
    /// Construct an emitter that pushes onto the given channel.
    pub(crate) fn new(tx: UnboundedSender<AuditWriterMessage>) -> Self {
        Self { tx: Some(tx) }
    }

    /// Construct a no-op emitter. Useful in tests, or where audit logging is
    /// disabled by configuration.
    pub fn noop() -> Self {
        Self { tx: None }
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
        if let Err(err) = tx.send(AuditWriterMessage::Event(Box::new(event))) {
            warn!(error = %err, "audit emitter: writer task dropped, audit event lost");
        }
    }

    /// Wait until every event queued before this call has been written.
    ///
    /// Returns false when no writer is configured or the writer has stopped.
    pub async fn flush(&self) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        if tx.send(AuditWriterMessage::Flush(ack_tx)).is_err() {
            return false;
        }
        ack_rx.await.is_ok()
    }

    pub(crate) fn shutdown_writer(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(AuditWriterMessage::Shutdown);
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
        let (tx, mut rx) = mpsc::unbounded_channel();
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
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        let emitter = AuditEmitter::new(tx);
        emitter.emit(
            AuditEventBuilder::new(AuditCategory::Api, "api.request", AuditOutcome::Success)
                .build(),
        );
    }
}
