use std::time::Duration;

use attune_common::{
    models::ManagerExecutionCompletion,
    mq::{ExecutionCompletedPayload, MessageEnvelope, MessageType, Publisher},
    repositories::manager_assignment::{
        CompletionPublishKind, ManagerAssignmentRepository, MANAGER_COMPLETION_SOURCE,
    },
};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::state::SharedState;

const PUBLICATION_BATCH_SIZE: usize = 32;
const REPLAYS_PER_TICK: usize = 32;
const PUBLICATIONS_PER_REPLAY_SLOT: usize = 8;

pub async fn publish_one(
    db: &PgPool,
    publisher: &Publisher,
    kind: CompletionPublishKind,
) -> attune_common::Result<bool> {
    let claim = Uuid::new_v4();
    let Some(record) = ManagerAssignmentRepository::claim_completion(db, claim, kind).await? else {
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
    let mut replay_budget = 0;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = interval.tick() => { replay_budget = REPLAYS_PER_TICK; }
            _ = state.manager_completion_wakeup.notified(), if !retry_on_tick => {}
        }
        let Some(publisher) = state.get_publisher().await else {
            continue;
        };

        let batch = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            result = publish_batch(&state.db, &publisher, &mut replay_budget) => result,
        };
        retry_on_tick = batch.is_err();
        match batch {
            // A notification can stand for more than one batch. Yield back to
            // the select to observe shutdown and refill the replay budget.
            Ok(true) => state.manager_completion_wakeup.notify_one(),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, "Manager completion publication deferred");
            }
        }
    }
}

async fn publish_batch(
    db: &PgPool,
    publisher: &Publisher,
    replay_budget: &mut usize,
) -> attune_common::Result<bool> {
    for index in 0..PUBLICATION_BATCH_SIZE {
        // Give replays a turn even if new completions keep arriving.
        if index % PUBLICATIONS_PER_REPLAY_SLOT == PUBLICATIONS_PER_REPLAY_SLOT - 1
            && publish_replay(db, publisher, replay_budget).await
        {
            continue;
        }
        if publish_one(db, publisher, CompletionPublishKind::Initial).await? {
            continue;
        }
        if !publish_replay(db, publisher, replay_budget).await {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn publish_replay(db: &PgPool, publisher: &Publisher, budget: &mut usize) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    match publish_one(db, publisher, CompletionPublishKind::Replay).await {
        Ok(true) => true,
        result => {
            *budget = 0;
            if let Err(error) = result {
                tracing::warn!(%error, "Manager completion replay deferred");
            }
            false
        }
    }
}
