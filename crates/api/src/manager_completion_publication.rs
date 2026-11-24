use std::time::Duration;

use attune_common::{
    models::ManagerExecutionCompletion,
    mq::{ExecutionCompletedPayload, MessageEnvelope, MessageType, Publisher},
    repositories::manager_assignment::{ManagerAssignmentRepository, MANAGER_COMPLETION_SOURCE},
};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::state::SharedState;

pub async fn publish_one(db: &PgPool, publisher: &Publisher) -> attune_common::Result<bool> {
    let claim = Uuid::new_v4();
    let Some(record) = ManagerAssignmentRepository::claim_completion(db, claim).await? else {
        return Ok(false);
    };

    let envelope = completion_envelope(&record);
    // A timeout has unknown publication outcome. A replacement publishes the
    // same event ID after the claim expires.
    tokio::time::timeout(
        Duration::from_secs(10),
        publisher.publish_required_envelope(&envelope),
    )
    .await
    .map_err(|_| {
        attune_common::Error::Timeout("Manager completion publication timed out".into())
    })??;
    ManagerAssignmentRepository::confirm_completion(db, record.assignment, claim).await?;
    Ok(true)
}

fn completion_envelope(
    record: &ManagerExecutionCompletion,
) -> MessageEnvelope<ExecutionCompletedPayload> {
    let mut envelope = MessageEnvelope::new(
        MessageType::ExecutionCompleted,
        ExecutionCompletedPayload {
            execution_id: record.execution,
            action_id: record.action,
            action_ref: record.action_ref.clone(),
            status: format!("{:?}", record.status),
            // The executor resolves the classified result from its persisted source.
            result: None,
            completed_at: record.completed_at,
        },
    )
    .with_source(MANAGER_COMPLETION_SOURCE);
    envelope.message_id = record.message_id;
    envelope.correlation_id = record.message_id;
    envelope.timestamp = record.completed_at;
    envelope
}

pub async fn run(
    state: SharedState,
    shutdown: CancellationToken,
    mut interval: tokio::time::Interval,
) {
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Pace failed attempts even when new commits continue to arrive.
    let mut retry_on_tick = false;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = state.manager_completion_wakeup.notified(), if !retry_on_tick => {}
            _ = interval.tick() => {}
        }
        let Some(publisher) = state.get_publisher().await else {
            continue;
        };

        retry_on_tick = false;
        loop {
            let published = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                result = publish_one(&state.db, &publisher) => result,
            };
            match published {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    tracing::warn!(%error, "Manager completion publication deferred");
                    retry_on_tick = true;
                    break;
                }
            }
        }
    }
}
