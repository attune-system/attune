//! Pack Test Processor
//!
//! Consumes `pack.test.requested` messages from the API and dispatches each
//! pack test run to a worker that supports the runtimes required by the pack's
//! test runners (e.g., python for unittest/pytest). This keeps test execution
//! on worker containers rather than the API container.

use anyhow::Result;
use attune_common::{
    models::{PackInstallStatus, Worker},
    mq::{Consumer, MessageEnvelope, MessageType, PackTestRequestedPayload, Publisher},
    repositories::PackInstallRepository,
    scheduling::{
        parse_worker_affinity, parse_worker_selector, parse_worker_tolerations, WorkerPlacement,
    },
};
use sqlx::PgPool;
use std::sync::{atomic::AtomicUsize, Arc};
use tracing::{error, info, warn};

use crate::scheduler::ExecutionScheduler;

/// Handles pack test dispatch requests
pub struct PackTestProcessor {
    pool: PgPool,
    publisher: Arc<Publisher>,
    consumer: Arc<Consumer>,
    round_robin_counter: Arc<AtomicUsize>,
}

impl PackTestProcessor {
    /// Create a new pack test processor
    pub fn new(pool: PgPool, publisher: Arc<Publisher>, consumer: Arc<Consumer>) -> Self {
        Self {
            pool,
            publisher,
            consumer,
            round_robin_counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Start consuming pack.test.requested messages
    pub async fn start(&self) -> Result<()> {
        info!("Starting pack test processor");

        let pool = self.pool.clone();
        let publisher = self.publisher.clone();
        let counter = self.round_robin_counter.clone();

        self.consumer
            .consume_with_handler(move |envelope: MessageEnvelope<PackTestRequestedPayload>| {
                let pool = pool.clone();
                let publisher = publisher.clone();
                let counter = counter.clone();

                async move {
                    if let Err(e) =
                        Self::dispatch_pack_test(&pool, &publisher, &counter, &envelope.payload)
                            .await
                    {
                        error!(
                            "Failed to dispatch pack test for install {} ({}): {}",
                            envelope.payload.pack_install_id, envelope.payload.pack_ref, e
                        );
                        return Err(format!("Failed to dispatch pack test: {}", e).into());
                    }
                    Ok(())
                }
            })
            .await?;

        Ok(())
    }

    /// Select a worker and dispatch the pack test to that worker's queue.
    async fn dispatch_pack_test(
        pool: &PgPool,
        publisher: &Publisher,
        counter: &AtomicUsize,
        payload: &PackTestRequestedPayload,
    ) -> Result<()> {
        let worker = match Self::select_worker(pool, payload, counter).await {
            Ok(worker) => worker,
            Err(e) => {
                warn!(
                    "No worker available for pack '{}' test run: {}",
                    payload.pack_ref, e
                );
                Self::mark_install_failed(pool, payload, &e.to_string()).await;
                return Ok(());
            }
        };

        info!(
            "Dispatching pack '{}' (v{}) test run (install {}) to worker {} ({})",
            payload.pack_ref, payload.pack_version, payload.pack_install_id, worker.id, worker.name
        );

        // Mark the install record as running now that a worker is selected.
        if let Err(e) = PackInstallRepository::new(pool.clone())
            .mark_running(payload.pack_install_id)
            .await
        {
            warn!(
                "Failed to mark pack install {} as running: {}",
                payload.pack_install_id, e
            );
        }

        let envelope = MessageEnvelope::new(MessageType::PackTestRequested, payload.clone())
            .with_source("executor");

        let routing_key = format!("pack.test.dispatch.worker.{}", worker.id);
        publisher
            .publish_envelope_with_routing(&envelope, "attune.executions", &routing_key)
            .await?;

        info!(
            "Published pack test message to worker {} (routing key: {})",
            worker.id, routing_key
        );

        Ok(())
    }

    /// Pick an active, fresh worker supporting the required runtimes.
    async fn select_worker(
        pool: &PgPool,
        payload: &PackTestRequestedPayload,
        counter: &AtomicUsize,
    ) -> Result<Worker> {
        // Prefer explicit required runtimes when present.
        let placement = WorkerPlacement {
            selector: parse_worker_selector(&payload.worker_selector)?,
            tolerations: parse_worker_tolerations(&payload.worker_tolerations)?,
            affinity: parse_worker_affinity(&payload.worker_affinity)?,
        };
        if !payload.required_runtimes.is_empty() {
            return ExecutionScheduler::select_worker_for_pack_test(
                pool,
                &payload.required_runtimes,
                placement,
                counter,
            )
            .await;
        }

        // Fall back to any active action worker.
        ExecutionScheduler::select_worker_for_pack_test(pool, &[], placement, counter).await
    }

    /// Mark the pack install record as failed when no worker is available.
    async fn mark_install_failed(pool: &PgPool, payload: &PackTestRequestedPayload, message: &str) {
        let repo = PackInstallRepository::new(pool.clone());
        if let Err(e) = repo
            .update_status(
                payload.pack_install_id,
                PackInstallStatus::Failed,
                Some(format!(
                    "Could not dispatch pack tests: no worker supports required runtimes ({}). {}",
                    payload.required_runtimes.join(", "),
                    message
                )),
            )
            .await
        {
            error!(
                "Failed to record pack install {} failure: {}",
                payload.pack_install_id, e
            );
        }
    }
}
