//! Pack Test Processor
//!
//! Consumes `pack.test.requested` messages from the API and dispatches each
//! pack test run to a worker that supports the runtimes required by the pack's
//! test runners (e.g., python for unittest/pytest). This keeps test execution
//! on worker containers rather than the API container.

use anyhow::Result;
use attune_common::{
    auth::generate_integration_token,
    models::{PackInstallStatus, Worker},
    mq::{
        Consumer, MessageEnvelope, MessageType, MqError, MqResult, PackTestRequestedPayload,
        Publisher,
    },
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
                        return Err(e);
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
    ) -> MqResult<()> {
        let repo = PackInstallRepository::new(pool.clone());
        let Some(current) = repo
            .find_by_id(payload.pack_install_id)
            .await
            .map_err(|error| MqError::Pool(error.to_string()))?
        else {
            info!(
                "Skipping dispatch for missing pack install {}",
                payload.pack_install_id
            );
            return Ok(());
        };
        if current.status != PackInstallStatus::Pending.as_str() {
            info!(
                status = %current.status,
                "Skipping duplicate dispatch for pack install {}",
                payload.pack_install_id
            );
            return Ok(());
        }
        if current.pack_ref != payload.pack_ref
            || current.pack_version != payload.pack_version
            || current.trigger_reason != payload.trigger_reason
        {
            let _ = repo
                .finish_pending(
                    payload.pack_install_id,
                    "Pack test request did not match its install record".to_string(),
                )
                .await;
            warn!(
                pack_install_id = payload.pack_install_id,
                "Rejected pack test request that did not match its install record"
            );
            return Ok(());
        }

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

        let candidate_token = payload
            .candidate_path
            .as_ref()
            .map(|_| generate_integration_token())
            .transpose()
            .map_err(|error| MqError::Other(error.to_string()))?;
        let Some(install) = repo
            .claim_worker(
                payload.pack_install_id,
                worker.id,
                candidate_token.as_ref().map(|token| token.hash.as_str()),
            )
            .await
            .map_err(|error| MqError::Pool(error.to_string()))?
        else {
            info!(
                "Skipping dispatch for already claimed pack install {}",
                payload.pack_install_id
            );
            return Ok(());
        };
        let assigned_worker_id = install.assigned_worker_id.ok_or_else(|| {
            MqError::Pool(format!(
                "Pack install {} entered running state without a worker assignment",
                payload.pack_install_id
            ))
        })?;

        info!(
            "Dispatching pack '{}' (v{}) test run (install {}) to worker {}",
            payload.pack_ref, payload.pack_version, payload.pack_install_id, assigned_worker_id
        );

        let mut worker_payload = payload.clone();
        worker_payload.candidate_access_token = candidate_token.map(|token| token.secret);
        let envelope = MessageEnvelope::new(MessageType::PackTestRequested, worker_payload)
            .with_source("executor");

        let routing_key = format!("pack.test.dispatch.worker.{}", assigned_worker_id);
        if let Err(error) = publisher
            .publish_envelope_with_routing(&envelope, "attune.executions", &routing_key)
            .await
        {
            let _ = repo
                .finish_running(
                    payload.pack_install_id,
                    PackInstallStatus::Failed,
                    None,
                    None,
                    Some(format!("Failed to dispatch pack tests to worker: {error}")),
                )
                .await;
            return Err(error);
        }

        info!(
            "Published pack test message to worker {} (routing key: {})",
            assigned_worker_id, routing_key
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
            .finish_pending(
                payload.pack_install_id,
                format!(
                    "Could not dispatch pack tests: no worker supports required runtimes ({}). {}",
                    payload.required_runtimes.join(", "),
                    message
                ),
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
