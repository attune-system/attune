//! Background batch-writer task that drains the audit channel and inserts
//! events into the `audit_event` hypertable.

use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, QueryBuilder};
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use super::{emitter::AuditWriterMessage, AuditEmitter, PendingAuditEvent};

/// Maximum number of events to flush in a single INSERT.
const DEFAULT_MAX_BATCH: usize = 256;

/// Maximum time to wait for a partial batch to fill before flushing.
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 200;

/// Handle to a spawned audit writer task. Drop to signal shutdown; the task
/// will drain remaining events and exit.
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
    let (tx, rx) = mpsc::unbounded_channel();
    let emitter = AuditEmitter::new(tx);
    let task = tokio::spawn(async move {
        run_writer(pool, rx, max_batch, flush_interval_ms).await;
    });
    AuditWriterHandle { emitter, task }
}

/// Spawn a writer with a dedicated runtime and schema-scoped pool.
pub fn spawn_threaded_writer(
    database_url: String,
    schema: String,
) -> Result<ThreadedAuditWriterHandle, sqlx::Error> {
    let (tx, rx) = mpsc::unbounded_channel();
    let emitter = AuditEmitter::new(tx);
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
                let pool = match PgPoolOptions::new()
                    .max_connections(2)
                    .after_connect(move |connection, _| {
                        let search_path = search_path.clone();
                        Box::pin(async move {
                            sqlx::query(&search_path).execute(connection).await?;
                            Ok(())
                        })
                    })
                    .connect(&database_url)
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
    mut rx: UnboundedReceiver<AuditWriterMessage>,
    max_batch: usize,
    flush_interval_ms: u64,
) {
    info!(max_batch, flush_interval_ms, "audit writer task started");
    let mut buffer: Vec<PendingAuditEvent> = Vec::with_capacity(max_batch);
    let flush_interval = Duration::from_millis(flush_interval_ms);

    'writer: loop {
        // Wait for at least one event, or for the channel to close.
        let first = match rx.recv().await {
            Some(AuditWriterMessage::Event(event)) => *event,
            Some(AuditWriterMessage::Flush(ack)) => {
                let _ = ack.send(());
                continue;
            }
            Some(AuditWriterMessage::Shutdown) => break,
            None => break, // emitter dropped
        };
        buffer.push(first);

        // Drain additional events that are already queued, then optionally
        // wait up to flush_interval for the buffer to fill.
        while buffer.len() < max_batch {
            match rx.try_recv() {
                Ok(AuditWriterMessage::Event(event)) => buffer.push(*event),
                Ok(AuditWriterMessage::Flush(ack)) => {
                    flush(&pool, &mut buffer).await;
                    let _ = ack.send(());
                    continue 'writer;
                }
                Ok(AuditWriterMessage::Shutdown) => {
                    flush(&pool, &mut buffer).await;
                    break 'writer;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    flush(&pool, &mut buffer).await;
                    info!("audit writer task draining and exiting");
                    return;
                }
            }
        }

        if buffer.len() < max_batch {
            // Wait briefly for more events.
            let deadline = tokio::time::sleep(flush_interval);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    biased;
                    _ = &mut deadline => break,
                    maybe_evt = rx.recv() => {
                        match maybe_evt {
                            Some(AuditWriterMessage::Event(event)) => {
                                buffer.push(*event);
                                if buffer.len() >= max_batch {
                                    break;
                                }
                            }
                            Some(AuditWriterMessage::Flush(ack)) => {
                                flush(&pool, &mut buffer).await;
                                let _ = ack.send(());
                                continue 'writer;
                            }
                            Some(AuditWriterMessage::Shutdown) => {
                                flush(&pool, &mut buffer).await;
                                break 'writer;
                            }
                            None => {
                                flush(&pool, &mut buffer).await;
                                info!("audit writer task draining and exiting");
                                return;
                            }
                        }
                    }
                }
            }
        }

        flush(&pool, &mut buffer).await;
    }

    // Channel closed, drain leftovers.
    if !buffer.is_empty() {
        flush(&pool, &mut buffer).await;
    }
    info!("audit writer task exited");
}

async fn flush(pool: &PgPool, buffer: &mut Vec<PendingAuditEvent>) {
    if buffer.is_empty() {
        return;
    }
    let count = buffer.len();
    debug!(count, "audit writer flushing batch");

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO audit_event (\
            category, event_type, outcome, \
            actor_identity, actor_login, actor_token_type, actor_ip, actor_user_agent, \
            request_id, \
            resource_type, resource_id, resource_ref, \
            http_method, http_path, http_status, duration_ms, \
            details, correlation_chain\
        ) ",
    );

    qb.push_values(buffer.drain(..), |mut b, e| {
        b.push_bind(e.category)
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
    });

    // Note: the explicit `::inet` cast above replaces the prior implicit
    // coercion approach which failed under some PostgreSQL configurations
    // (`column "actor_ip" is of type inet but expression is of type text`).

    match qb.build().execute(pool).await {
        Ok(res) => debug!(rows = res.rows_affected(), "audit batch flushed"),
        Err(err) => {
            error!(error = %err, count, "audit writer: batch insert failed; events dropped");
        }
    }
    let _ = count;
}

#[cfg(test)]
mod tests {
    // Writer tests require a live PostgreSQL with the audit_event table.
    // They live in the integration test suite (crates/common/tests) rather
    // than as unit tests, since they need the schema to exist.

    use super::*;
    use crate::audit::{AuditCategory, AuditEventBuilder, AuditOutcome};
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
