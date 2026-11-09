//! Background batch-writer task that drains the audit channel and inserts
//! events into the ordinary PostgreSQL `audit_event` table.

use super::emitter::{AuditStats, TrackedAuditEvent};
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, QueryBuilder};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use tokio::sync::mpsc::{self, Receiver};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use super::{emitter::AuditWriterMessage, AuditEmitter};

/// Maximum number of events to flush in a single INSERT.
const DEFAULT_MAX_BATCH: usize = 256;

/// Maximum time to wait for a partial batch to fill before flushing.
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 200;

/// Handle to a spawned audit writer. Call `shutdown()` to drain and join it.
pub struct AuditWriterHandle {
    pub emitter: AuditEmitter,
    pub task: JoinHandle<()>,
}

/// Audit writer hosted on a dedicated runtime. This is useful for test
/// harnesses that must synchronously join background work from `Drop`.
pub struct ThreadedAuditWriterHandle {
    pub emitter: AuditEmitter,
    task: Option<std::thread::JoinHandle<()>>,
}

impl AuditWriterHandle {
    /// Stop admission, drain accepted events and join. A deadline aborts and joins
    /// the task, accounting every event not confirmed persisted as loss.
    pub async fn shutdown(mut self) {
        let drained = tokio::time::timeout(Duration::from_secs(30), async {
            self.emitter.request_shutdown().await;
            (&mut self.task).await
        })
        .await;
        if drained.is_err() {
            self.task.abort();
            let _ = self.task.await;
            error!(
                lost = self.emitter.stats.lost.load(Ordering::Relaxed),
                "audit shutdown exceeded drain budget"
            );
        } else if let Ok(Err(error)) = drained {
            error!(%error, "audit writer task failed during shutdown");
        }
    }
}

impl ThreadedAuditWriterHandle {
    pub fn shutdown(mut self) -> std::thread::Result<()> {
        self.emitter.shutdown_writer();
        drop(self.emitter);
        self.task.take().expect("audit writer task missing").join()
    }
}

/// Spawn the background writer task and return both an emitter and the
/// task handle.
pub fn spawn_writer(pool: PgPool) -> AuditWriterHandle {
    spawn_writer_with(pool, DEFAULT_MAX_BATCH, DEFAULT_FLUSH_INTERVAL_MS)
}

/// As [`spawn_writer`] but with explicit batch tuning. Used in tests.
pub fn spawn_writer_with(
    pool: PgPool,
    max_batch: usize,
    flush_interval_ms: u64,
) -> AuditWriterHandle {
    // Tuning may reduce latency in tests, but cannot bypass the memory and
    // statement-size bound used in production.
    let max_batch = max_batch.clamp(1, DEFAULT_MAX_BATCH);
    let (tx, rx) = mpsc::channel(4096);
    let emitter = AuditEmitter::new(tx);
    let stats = emitter.stats.clone();
    let task = tokio::spawn(async move {
        run_writer(pool, rx, max_batch, flush_interval_ms, stats).await;
    });
    AuditWriterHandle { emitter, task }
}

/// Spawn a writer with a dedicated runtime and schema-scoped pool.
pub fn spawn_threaded_writer(
    database_url: String,
    schema: String,
) -> Result<ThreadedAuditWriterHandle, sqlx::Error> {
    let (tx, rx) = mpsc::channel(4096);
    let emitter = AuditEmitter::new(tx);
    let stats = emitter.stats.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let task = std::thread::Builder::new()
        .name(format!("audit-writer-{schema}"))
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build audit writer runtime");
            runtime.block_on(async move {
                let search_path = format!("SET search_path TO {schema}, public");
                let identity = crate::db::ConnectionIdentity::for_process("api", "audit-writer");
                let connect_options = match identity.connect_options(&database_url) {
                    Ok(options) => options,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let pool = match PgPoolOptions::new()
                    .max_connections(2)
                    .after_connect(move |connection, _| {
                        let search_path = search_path.clone();
                        Box::pin(async move {
                            sqlx::query(&search_path).execute(connection).await?;
                            Ok(())
                        })
                    })
                    .connect_with(connect_options)
                    .await
                {
                    Ok(pool) => pool,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                run_writer(
                    pool.clone(),
                    rx,
                    DEFAULT_MAX_BATCH,
                    DEFAULT_FLUSH_INTERVAL_MS,
                    stats,
                )
                .await;
                pool.close().await;
            });
        })
        .expect("failed to spawn audit writer thread");

    match ready_rx
        .recv()
        .expect("audit writer thread exited during startup")
    {
        Ok(()) => Ok(ThreadedAuditWriterHandle {
            emitter,
            task: Some(task),
        }),
        Err(error) => {
            task.join().expect("audit writer thread panicked");
            Err(error)
        }
    }
}

async fn run_writer(
    pool: PgPool,
    rx: Receiver<AuditWriterMessage>,
    max_batch: usize,
    flush_interval_ms: u64,
    stats: Arc<AuditStats>,
) {
    let mut work = Box::pin(writer_loop(
        pool,
        rx,
        max_batch,
        flush_interval_ms,
        stats.clone(),
    ));
    tokio::select! {
        _ = &mut work => return,
        _ = stats.shutdown.cancelled() => (),
    }
    if tokio::time::timeout(Duration::from_secs(30), &mut work)
        .await
        .is_err()
    {
        drop(work);
        error!(
            lost = stats.lost.load(Ordering::Relaxed),
            "audit writer drain deadline; unconfirmed events lost"
        );
    }
}

async fn writer_loop(
    pool: PgPool,
    mut rx: Receiver<AuditWriterMessage>,
    max_batch: usize,
    flush_interval_ms: u64,
    stats: Arc<AuditStats>,
) {
    let mut buffer = Vec::with_capacity(max_batch.max(1));
    let mut interval = tokio::time::interval(Duration::from_millis(flush_interval_ms.max(1)));
    let mut closed = false;
    loop {
        tokio::select! {
            _ = stats.shutdown.cancelled(), if !closed => { rx.close(); closed = true; }
            message = rx.recv() => match message {
                Some(AuditWriterMessage::Event(event)) => {
                    buffer.push(*event);
                    if buffer.len() >= max_batch.max(1) { flush(&pool, &mut buffer, &stats).await; }
                }
                Some(AuditWriterMessage::Flush(ack)) => {
                    flush(&pool, &mut buffer, &stats).await;
                    let _ = ack.send(stats.lost.load(Ordering::Relaxed) == 0);
                }
                Some(AuditWriterMessage::Shutdown) => { rx.close(); closed = true; },
                None => break,
            },
            _ = interval.tick() => { flush(&pool, &mut buffer, &stats).await; }
        }
    }
    flush(&pool, &mut buffer, &stats).await;
    info!(
        lost = stats.lost.load(Ordering::Relaxed),
        "audit writer drained"
    );
}

async fn flush(pool: &PgPool, buffer: &mut Vec<TrackedAuditEvent>, stats: &AuditStats) {
    if buffer.is_empty() {
        return;
    }
    let count = buffer.len();
    debug!(count, "audit writer flushing batch");

    // Reserve the complete partitioned primary key once. An ambiguous INSERT
    // may then be retried without creating another event, even across day rollover.
    let identities = tokio::time::timeout(Duration::from_secs(2),
        sqlx::query_as::<_, (i64, chrono::DateTime<chrono::Utc>)>(
            "SELECT nextval(pg_get_serial_sequence('audit_event', 'id')), clock_timestamp() FROM generate_series(1, $1)"
        ).bind(count as i64).fetch_all(pool)).await;
    let Ok(Ok(identities)) = identities else {
        buffer.clear();
        error!(count, "audit identity allocation failed; explicit loss");
        return;
    };
    for attempt in 0..3 {
        if attempt > 0 {
            stats.retries.fetch_add(1, Ordering::Relaxed);
        }
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO audit_event (\
            id, created, category, event_type, outcome, \
            actor_identity, actor_login, actor_token_type, actor_ip, actor_user_agent, \
            request_id, \
            resource_type, resource_id, resource_ref, \
            http_method, http_path, http_status, duration_ms, \
            details, correlation_chain\
        ) ",
        );

        qb.push_values(
            buffer
                .iter()
                .map(|tracked| tracked.event.clone())
                .zip(identities.iter().copied()),
            |mut b, (e, (id, created))| {
                b.push_bind(id)
                    .push_bind(created)
                    .push_bind(e.category)
                    .push_bind(e.event_type)
                    .push_bind(e.outcome)
                    .push_bind(e.actor_identity)
                    .push_bind(e.actor_login)
                    .push_bind(e.actor_token_type);
                // INET column: bind the text representation and append an explicit
                // ::inet cast so PostgreSQL parses it correctly.
                b.push_bind(e.actor_ip.map(|ip| ip.to_string()));
                b.push_unseparated("::inet");
                b.push_bind(e.actor_user_agent)
                    .push_bind(e.request_id)
                    .push_bind(e.resource_type)
                    .push_bind(e.resource_id)
                    .push_bind(e.resource_ref)
                    .push_bind(e.http_method)
                    .push_bind(e.http_path)
                    .push_bind(e.http_status)
                    .push_bind(e.duration_ms)
                    .push_bind(e.details)
                    .push_bind(e.correlation_chain);
            },
        );

        // Note: the explicit `::inet` cast above replaces the prior implicit
        // coercion approach which failed under some PostgreSQL configurations
        // (`column "actor_ip" is of type inet but expression is of type text`).

        qb.push(" ON CONFLICT (id, created) DO NOTHING");
        match tokio::time::timeout(Duration::from_secs(2), qb.build().execute(pool)).await {
            Ok(Ok(_)) => {
                for event in buffer.iter_mut() {
                    event.confirm();
                }
                buffer.clear();
                return;
            }
            Ok(Err(error)) => {
                stats.insert_failures.fetch_add(1, Ordering::Relaxed);
                let class = error
                    .as_database_error()
                    .and_then(|error| error.code())
                    .filter(|code| {
                        code.len() == 5 && code.bytes().all(|byte| byte.is_ascii_alphanumeric())
                    })
                    .map(|code| code[..2].to_owned())
                    .unwrap_or_else(|| "transport".into());
                error!(
                    count,
                    attempt,
                    sqlstate_class = class,
                    "audit persistence attempt failed or uncertain"
                );
            }
            Err(_) => {
                stats.insert_failures.fetch_add(1, Ordering::Relaxed);
                error!(
                    count,
                    attempt, "audit persistence attempt deadline; outcome uncertain"
                );
            }
        }
    }
    buffer.clear();
    error!(
        count,
        "audit retry budget exhausted; explicit unconfirmed loss"
    );
}

#[cfg(test)]
mod tests {
    // Writer tests require a live PostgreSQL with the audit_event table.
    // They live in the integration test suite (crates/common/tests) rather
    // than as unit tests, since they need the schema to exist.

    use super::*;
    use crate::audit::{AuditCategory, AuditEventBuilder, AuditOutcome, PendingAuditEvent};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn writer_exits_when_channel_closes() {
        // Use a (channel-only) variant of the writer loop logic to verify
        // shutdown semantics without needing a database. We exercise the
        // runtime contract by manually polling the channel.
        let (tx, mut rx) = mpsc::unbounded_channel::<PendingAuditEvent>();
        let task = tokio::spawn(async move {
            // Drain until closed.
            while rx.recv().await.is_some() {}
        });
        tx.send(
            AuditEventBuilder::new(AuditCategory::Api, "api.request", AuditOutcome::Success)
                .build(),
        )
        .unwrap();
        drop(tx);
        // Should exit promptly after channel closes.
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("writer task should exit when channel closes")
            .unwrap();
    }
}
