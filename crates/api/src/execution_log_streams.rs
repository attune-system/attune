use std::{
    collections::HashMap,
    fmt::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ExecutionLogStreams {
    inner: Arc<Inner>,
}

struct Inner {
    global_limit: usize,
    per_identity_limit: usize,
    counts: Mutex<Counts>,
    active: AtomicU64,
    wakeups: AtomicU64,
    retries: AtomicU64,
    database_queries: AtomicU64,
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

pub struct ExecutionLogStreamPermit {
    inner: Arc<Inner>,
    identity_id: i64,
}

impl ExecutionLogStreams {
    pub fn new(global_limit: usize, per_identity_limit: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                global_limit,
                per_identity_limit,
                counts: Mutex::new(Counts::default()),
                active: AtomicU64::new(0),
                wakeups: AtomicU64::new(0),
                retries: AtomicU64::new(0),
                database_queries: AtomicU64::new(0),
                object_store_reads: AtomicU64::new(0),
                shutdown: CancellationToken::new(),
            }),
        }
    }

    pub fn try_acquire(&self, identity_id: i64) -> Result<ExecutionLogStreamPermit, LimitExceeded> {
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
        self.inner.active.fetch_add(1, Ordering::Relaxed);
        Ok(ExecutionLogStreamPermit {
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

    pub fn record_database_query(&self) {
        self.inner.database_queries.fetch_add(1, Ordering::Relaxed);
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
                "Active execution log SSE streams.",
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
                "attune_execution_log_stream_database_queries_total",
                "counter",
                "Database queries made while serving execution log streams.",
                self.inner.database_queries.load(Ordering::Relaxed),
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

impl Drop for ExecutionLogStreamPermit {
    fn drop(&mut self) {
        let mut counts = self
            .inner
            .counts
            .lock()
            .expect("stream counts lock poisoned");
        counts.global -= 1;
        let identity_count = counts
            .by_identity
            .get_mut(&self.identity_id)
            .expect("permit identity count missing");
        *identity_count -= 1;
        if *identity_count == 0 {
            counts.by_identity.remove(&self.identity_id);
        }
        self.inner.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_global_limit_and_releases_on_disconnect() {
        let streams = ExecutionLogStreams::new(2, 2);
        let first = streams.try_acquire(1).unwrap();
        let second = streams.try_acquire(2).unwrap();
        assert!(matches!(streams.try_acquire(3), Err(LimitExceeded::Global)));

        drop(first);
        assert!(streams.try_acquire(3).is_ok());
        drop(second);
    }

    #[test]
    fn scopes_limits_by_identity() {
        let streams = ExecutionLogStreams::new(3, 1);
        let first = streams.try_acquire(1).unwrap();
        assert!(matches!(
            streams.try_acquire(1),
            Err(LimitExceeded::Identity)
        ));
        assert!(streams.try_acquire(2).is_ok());

        drop(first);
        assert!(streams.try_acquire(1).is_ok());
    }

    #[test]
    fn reports_stream_metrics() {
        let streams = ExecutionLogStreams::new(2, 2);
        let permit = streams.try_acquire(1).unwrap();
        streams.record_wakeup();
        streams.record_retry();
        streams.record_database_query();
        streams.record_object_store_read();

        let metrics = streams.render_metrics();
        for metric in [
            "attune_execution_log_streams_active 1",
            "attune_execution_log_stream_wakeups_total 1",
            "attune_execution_log_stream_retries_total 1",
            "attune_execution_log_stream_database_queries_total 1",
            "attune_execution_log_stream_object_store_reads_total 1",
        ] {
            assert!(metrics.contains(metric));
        }
        drop(permit);
        assert!(streams
            .render_metrics()
            .contains("attune_execution_log_streams_active 0"));
    }
}
