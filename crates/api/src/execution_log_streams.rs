use std::{
    collections::HashMap,
    fmt::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use attune_common::repositories::execution_log_stream_lease::{
    ExecutionLogStreamAdmission, ExecutionLogStreamLeaseRepository,
};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub struct ExecutionLogStreams {
    inner: Arc<Inner>,
}

struct Inner {
    global_limit: usize,
    per_identity_limit: usize,
    lease_seconds: u64,
    heartbeat_seconds: u64,
    counts: Mutex<Counts>,
    active: AtomicU64,
    wakeups: AtomicU64,
    retries: AtomicU64,
    tail_database_queries: AtomicU64,
    object_store_reads: AtomicU64,
    shutdown: CancellationToken,
}

#[derive(Default)]
struct Counts {
    global: usize,
    by_identity: HashMap<i64, usize>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LimitExceeded {
    Global,
    Identity,
}

#[derive(Debug, thiserror::Error)]
pub enum AdmissionError {
    #[error("execution log stream limit exceeded: {0:?}")]
    Limit(LimitExceeded),
    #[error(transparent)]
    Repository(#[from] attune_common::Error),
}

pub struct ExecutionLogStreamPermit {
    inner: Arc<Inner>,
    _local_permit: LocalPermit,
    db: PgPool,
    lease_id: Uuid,
    heartbeat_stop: CancellationToken,
    lease_lost: CancellationToken,
}

struct LocalPermit {
    inner: Arc<Inner>,
    identity_id: i64,
}

impl ExecutionLogStreams {
    pub fn new(
        global_limit: usize,
        per_identity_limit: usize,
        lease_seconds: u64,
        heartbeat_seconds: u64,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                global_limit,
                per_identity_limit,
                lease_seconds,
                heartbeat_seconds,
                counts: Mutex::new(Counts::default()),
                active: AtomicU64::new(0),
                wakeups: AtomicU64::new(0),
                retries: AtomicU64::new(0),
                tail_database_queries: AtomicU64::new(0),
                object_store_reads: AtomicU64::new(0),
                shutdown: CancellationToken::new(),
            }),
        }
    }

    pub async fn acquire(
        &self,
        db: &PgPool,
        identity_id: i64,
    ) -> Result<ExecutionLogStreamPermit, AdmissionError> {
        let local_permit = self
            .try_acquire_local(identity_id)
            .map_err(AdmissionError::Limit)?;
        let admission = ExecutionLogStreamLeaseRepository::acquire(
            db,
            identity_id,
            self.inner.global_limit,
            self.inner.per_identity_limit,
            self.inner.lease_seconds,
        )
        .await?;
        let lease_id = match admission {
            ExecutionLogStreamAdmission::Acquired { lease_id, .. } => lease_id,
            ExecutionLogStreamAdmission::GlobalLimit => {
                return Err(AdmissionError::Limit(LimitExceeded::Global));
            }
            ExecutionLogStreamAdmission::IdentityLimit => {
                return Err(AdmissionError::Limit(LimitExceeded::Identity));
            }
        };

        let heartbeat_stop = CancellationToken::new();
        let lease_lost = CancellationToken::new();
        spawn_lease_heartbeat(
            db.clone(),
            lease_id,
            self.inner.lease_seconds,
            self.inner.heartbeat_seconds,
            heartbeat_stop.clone(),
            lease_lost.clone(),
        );
        self.inner.active.fetch_add(1, Ordering::Relaxed);
        Ok(ExecutionLogStreamPermit {
            inner: Arc::clone(&self.inner),
            _local_permit: local_permit,
            db: db.clone(),
            lease_id,
            heartbeat_stop,
            lease_lost,
        })
    }

    fn try_acquire_local(&self, identity_id: i64) -> Result<LocalPermit, LimitExceeded> {
        let mut counts = self
            .inner
            .counts
            .lock()
            .expect("stream counts lock poisoned");
        if counts.global >= self.inner.global_limit {
            return Err(LimitExceeded::Global);
        }
        let identity_count = counts.by_identity.entry(identity_id).or_default();
        if *identity_count >= self.inner.per_identity_limit {
            return Err(LimitExceeded::Identity);
        }
        *identity_count += 1;
        counts.global += 1;
        Ok(LocalPermit {
            inner: Arc::clone(&self.inner),
            identity_id,
        })
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }

    pub fn begin_shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    pub fn record_wakeup(&self) {
        self.inner.wakeups.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_retry(&self) {
        self.inner.retries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tail_database_query(&self) {
        self.inner
            .tail_database_queries
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_object_store_read(&self) {
        self.inner
            .object_store_reads
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn render_metrics(&self) -> String {
        let mut output = String::new();
        for (name, kind, help, value) in [
            (
                "attune_execution_log_streams_active",
                "gauge",
                "Active execution log SSE streams on this API replica.",
                self.inner.active.load(Ordering::Relaxed),
            ),
            (
                "attune_execution_log_stream_wakeups_total",
                "counter",
                "Execution log stream wakeups.",
                self.inner.wakeups.load(Ordering::Relaxed),
            ),
            (
                "attune_execution_log_stream_retries_total",
                "counter",
                "Execution log stream resume requests.",
                self.inner.retries.load(Ordering::Relaxed),
            ),
            (
                "attune_execution_log_stream_tail_database_queries_total",
                "counter",
                "Database queries issued by execution-log discovery, status reconciliation, stream refresh, and segment lookup; excludes admission, authorization, and shared-file snapshot queries.",
                self.inner.tail_database_queries.load(Ordering::Relaxed),
            ),
            (
                "attune_execution_log_stream_object_store_reads_total",
                "counter",
                "Object-store reads made while serving execution log streams.",
                self.inner.object_store_reads.load(Ordering::Relaxed),
            ),
        ] {
            writeln!(output, "# HELP {name} {help}").expect("writing to String cannot fail");
            writeln!(output, "# TYPE {name} {kind}").expect("writing to String cannot fail");
            writeln!(output, "{name} {value}").expect("writing to String cannot fail");
        }
        output
    }
}

impl ExecutionLogStreamPermit {
    pub fn lease_lost_token(&self) -> CancellationToken {
        self.lease_lost.clone()
    }
}

fn spawn_lease_heartbeat(
    db: PgPool,
    lease_id: Uuid,
    lease_seconds: u64,
    heartbeat_seconds: u64,
    stop: CancellationToken,
    lost: CancellationToken,
) {
    tokio::spawn(async move {
        let heartbeat = std::time::Duration::from_secs(heartbeat_seconds);
        let renewal_deadline = std::time::Duration::from_secs(
            lease_seconds
                .checked_sub(heartbeat_seconds)
                .expect("lease duration validated before server startup"),
        );
        let mut valid_until = tokio::time::Instant::now() + renewal_deadline;
        loop {
            let next_heartbeat = (tokio::time::Instant::now() + heartbeat).min(valid_until);
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep_until(next_heartbeat) => {}
            }
            if tokio::time::Instant::now() >= valid_until {
                tracing::error!(%lease_id, "Execution log stream lease renewal deadline elapsed");
                lost.cancel();
                return;
            }
            match tokio::time::timeout_at(
                valid_until,
                ExecutionLogStreamLeaseRepository::renew(&db, lease_id, lease_seconds),
            )
            .await
            {
                Ok(Ok(true)) => valid_until = tokio::time::Instant::now() + renewal_deadline,
                Ok(Ok(false)) => {
                    lost.cancel();
                    return;
                }
                Ok(Err(error)) if tokio::time::Instant::now() < valid_until => {
                    tracing::warn!(%error, %lease_id, "Execution log stream lease renewal failed; retrying");
                }
                Ok(Err(error)) => {
                    tracing::error!(%error, %lease_id, "Execution log stream lease renewal deadline elapsed after a database error");
                    lost.cancel();
                    return;
                }
                Err(_) => {
                    tracing::error!(%lease_id, "Execution log stream lease renewal timed out");
                    lost.cancel();
                    return;
                }
            }
        }
    });
}

impl Drop for ExecutionLogStreamPermit {
    fn drop(&mut self) {
        self.heartbeat_stop.cancel();
        let db = self.db.clone();
        let lease_id = self.lease_id;
        tokio::spawn(async move {
            if let Err(error) = ExecutionLogStreamLeaseRepository::release(&db, lease_id).await {
                tracing::warn!(%error, %lease_id, "Failed to release execution log stream lease");
            }
        });
        self.inner.active.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Drop for LocalPermit {
    fn drop(&mut self) {
        release_local(&self.inner, self.identity_id);
    }
}

fn release_local(inner: &Inner, identity_id: i64) {
    let mut counts = inner.counts.lock().expect("stream counts lock poisoned");
    counts.global -= 1;
    let identity_count = counts
        .by_identity
        .get_mut(&identity_id)
        .expect("permit identity count missing");
    *identity_count -= 1;
    if *identity_count == 0 {
        counts.by_identity.remove(&identity_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_optimization_enforces_global_limit_and_releases() {
        let streams = ExecutionLogStreams::new(2, 2, 45, 10);
        let first = streams.try_acquire_local(1).unwrap();
        let second = streams.try_acquire_local(2).unwrap();
        assert!(matches!(
            streams.try_acquire_local(3),
            Err(LimitExceeded::Global)
        ));

        drop(first);
        assert!(streams.try_acquire_local(3).is_ok());
        drop(second);
    }

    #[test]
    fn local_optimization_scopes_limits_by_identity() {
        let streams = ExecutionLogStreams::new(3, 1, 45, 10);
        let first = streams.try_acquire_local(1).unwrap();
        assert!(matches!(
            streams.try_acquire_local(1),
            Err(LimitExceeded::Identity)
        ));
        assert!(streams.try_acquire_local(2).is_ok());

        drop(first);
        assert!(streams.try_acquire_local(1).is_ok());
    }

    #[test]
    fn reports_stream_metrics_with_narrow_database_query_name() {
        let streams = ExecutionLogStreams::new(2, 2, 45, 10);
        streams.record_wakeup();
        streams.record_retry();
        streams.record_tail_database_query();
        streams.record_object_store_read();

        let metrics = streams.render_metrics();
        for metric in [
            "attune_execution_log_streams_active 0",
            "attune_execution_log_stream_wakeups_total 1",
            "attune_execution_log_stream_retries_total 1",
            "attune_execution_log_stream_tail_database_queries_total 1",
            "attune_execution_log_stream_object_store_reads_total 1",
        ] {
            assert!(metrics.contains(metric));
        }
        assert!(!metrics.contains("attune_execution_log_stream_database_queries_total"));
    }
}
