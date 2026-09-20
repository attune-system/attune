//! Inquiry response wake-up and database reconciliation.

use anyhow::Result;
use attune_common::{
    models::Id,
    mq::{Consumer, InquiryRespondedPayload, MessageEnvelope, Publisher},
    repositories::{
        inquiry::InquiryRepository, workflow_task_wait::WorkflowTaskWaitRepository, FindById,
    },
};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::scheduler::ExecutionScheduler;

pub struct InquiryHandler {
    pool: PgPool,
    publisher: Arc<Publisher>,
    consumer: Arc<Consumer>,
    encryption_key: Option<String>,
}

impl InquiryHandler {
    pub fn new(
        pool: PgPool,
        publisher: Arc<Publisher>,
        consumer: Arc<Consumer>,
        encryption_key: Option<String>,
    ) -> Self {
        Self {
            pool,
            publisher,
            consumer,
            encryption_key,
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!("Starting inquiry handler");
        let pool = self.pool.clone();
        let publisher = self.publisher.clone();
        let encryption_key = self.encryption_key.clone();

        self.consumer
            .consume_with_handler(move |envelope: MessageEnvelope<InquiryRespondedPayload>| {
                let pool = pool.clone();
                let publisher = publisher.clone();
                let encryption_key = encryption_key.clone();
                async move {
                    if let Err(error) = Self::handle_inquiry_response(
                        &pool,
                        &publisher,
                        encryption_key.as_deref(),
                        &envelope,
                    )
                    .await
                    {
                        error!("Error handling inquiry response: {}", error);
                        return Err(format!("Failed to handle inquiry response: {error}").into());
                    }
                    Ok(())
                }
            })
            .await?;
        Ok(())
    }

    async fn handle_inquiry_response(
        pool: &PgPool,
        publisher: &Publisher,
        encryption_key: Option<&str>,
        envelope: &MessageEnvelope<InquiryRespondedPayload>,
    ) -> Result<()> {
        let inquiry_id = envelope.payload.inquiry_id;
        let inquiry = InquiryRepository::find_by_id(pool, inquiry_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Inquiry {inquiry_id} not found"))?;
        if inquiry.workflow_execution.is_none() {
            warn!(
                "Ignoring response wake-up for unscoped inquiry {} after legacy inquiry removal",
                inquiry_id
            );
            return Ok(());
        }
        ExecutionScheduler::release_inquiry_waits(pool, publisher, inquiry_id, encryption_key).await
    }

    pub async fn check_inquiry_timeouts(pool: &PgPool) -> Result<Vec<Id>> {
        let timed_out = InquiryRepository::timeout_expired_pending(pool).await?;
        Ok(timed_out.into_iter().map(|inquiry| inquiry.id).collect())
    }

    pub async fn timeout_check_loop(
        pool: PgPool,
        publisher: Arc<Publisher>,
        encryption_key: Option<String>,
        interval_seconds: u64,
    ) {
        info!(
            "Starting inquiry timeout and wait reconciliation loop (interval: {}s)",
            interval_seconds
        );
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(interval_seconds));

        loop {
            interval.tick().await;
            if let Err(error) = Self::check_inquiry_timeouts(&pool).await {
                error!("Error checking inquiry timeouts: {}", error);
            }

            match WorkflowTaskWaitRepository::find_resolvable(&pool, 100).await {
                Ok(waits) => {
                    let mut targets = Vec::new();
                    for wait in waits {
                        if let Ok(target) = wait.target() {
                            if !targets.contains(&target) {
                                targets.push(target);
                            }
                        }
                    }
                    for target in targets {
                        if let Err(error) = ExecutionScheduler::release_target_waits(
                            &pool,
                            &publisher,
                            target,
                            encryption_key.as_deref(),
                        )
                        .await
                        {
                            error!("Error reconciling waits for target {:?}: {}", target, error);
                        }
                    }
                }
                Err(error) => error!("Error finding resolvable task waits: {}", error),
            }
        }
    }
}
