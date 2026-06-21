use anyhow::Result;
use attune_common::{
    models::{
        event::Event,
        work_queue::{WorkQueue, WorkQueueDispatch},
        Id, WorkQueueDispatchStatus, WorkQueueItemStatus,
    },
    mq::{EventCreatedPayload, MessageEnvelope, MessageType, Publisher},
    repositories::{
        event::CreateEventInput,
        work_queue::{
            WorkQueueDispatchRepository, WorkQueueDispatchSearchFilters, WorkQueueItemRepository,
            WorkQueueItemSearchFilters,
        },
        Create, EventRepository,
    },
};
use chrono::Utc;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::warn;

pub const CORE_QUEUE_STARTED_TRIGGER_REF: &str = "core.queue_started";
pub const CORE_QUEUE_EMPTY_TRIGGER_REF: &str = "core.queue_empty";

pub async fn maybe_emit_queue_started(
    pool: &PgPool,
    publisher: &Publisher,
    queue: &WorkQueue,
    dispatch_id: Id,
    execution_id: Id,
    leased_item_ids: &[Id],
) -> Result<()> {
    if latest_queue_lifecycle_event(pool, queue.id)
        .await?
        .as_deref()
        == Some(CORE_QUEUE_STARTED_TRIGGER_REF)
    {
        return Ok(());
    }

    emit_queue_lifecycle_event(
        pool,
        publisher,
        CORE_QUEUE_STARTED_TRIGGER_REF,
        queue,
        json!({
            "event_type": "queue_started",
            "queue_id": queue.id,
            "queue_ref": queue.r#ref.clone(),
            "pack_ref": queue.pack_ref.clone(),
            "dispatch_id": dispatch_id,
            "execution_id": execution_id,
            "dispatch_action_ref": queue.dispatch_action_ref.clone(),
            "leased_item_count": leased_item_ids.len(),
            "leased_item_ids": leased_item_ids,
            "observed_at": Utc::now(),
        }),
    )
    .await?;

    Ok(())
}

pub async fn maybe_emit_queue_empty(
    pool: &PgPool,
    publisher: &Publisher,
    queue: &WorkQueue,
    dispatch: &WorkQueueDispatch,
    execution_id: Id,
    leased_item_count: usize,
    terminal_dispatch_status: WorkQueueDispatchStatus,
) -> Result<()> {
    let active_dispatch_count = active_dispatch_count(pool, queue.id).await?;
    if active_dispatch_count > 0 {
        return Ok(());
    }

    let ready_item_count = ready_item_count(pool, queue.id).await?;
    if ready_item_count > 0 {
        return Ok(());
    }

    if latest_queue_lifecycle_event(pool, queue.id)
        .await?
        .as_deref()
        == Some(CORE_QUEUE_EMPTY_TRIGGER_REF)
    {
        return Ok(());
    }

    emit_queue_lifecycle_event(
        pool,
        publisher,
        CORE_QUEUE_EMPTY_TRIGGER_REF,
        queue,
        json!({
            "event_type": "queue_empty",
            "queue_id": queue.id,
            "queue_ref": queue.r#ref.clone(),
            "pack_ref": queue.pack_ref.clone(),
            "dispatch_id": dispatch.id,
            "execution_id": execution_id,
            "dispatch_action_ref": queue.dispatch_action_ref.clone(),
            "leased_item_count": leased_item_count,
            "terminal_dispatch_status": terminal_dispatch_status,
            "active_dispatch_count": active_dispatch_count,
            "ready_item_count": ready_item_count,
            "observed_at": Utc::now(),
        }),
    )
    .await?;

    Ok(())
}

async fn latest_queue_lifecycle_event(pool: &PgPool, queue_id: Id) -> Result<Option<String>> {
    sqlx::query_scalar(
        r#"
        SELECT trigger_ref
        FROM event
        WHERE trigger_ref = ANY($1::text[])
          AND payload->>'queue_id' = $2
        ORDER BY created DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(vec![
        CORE_QUEUE_STARTED_TRIGGER_REF.to_string(),
        CORE_QUEUE_EMPTY_TRIGGER_REF.to_string(),
    ])
    .bind(queue_id.to_string())
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

async fn active_dispatch_count(pool: &PgPool, queue_id: Id) -> Result<u64> {
    Ok(WorkQueueDispatchRepository::search(
        pool,
        &WorkQueueDispatchSearchFilters {
            queue: Some(queue_id),
            statuses: Some(vec![
                WorkQueueDispatchStatus::Leased,
                WorkQueueDispatchStatus::Dispatched,
            ]),
            limit: 1,
            ..Default::default()
        },
    )
    .await?
    .total)
}

async fn ready_item_count(pool: &PgPool, queue_id: Id) -> Result<u64> {
    Ok(WorkQueueItemRepository::search(
        pool,
        &WorkQueueItemSearchFilters {
            queue: Some(queue_id),
            statuses: Some(vec![
                WorkQueueItemStatus::Queued,
                WorkQueueItemStatus::Retry,
            ]),
            limit: 1,
            ..Default::default()
        },
    )
    .await?
    .total)
}

async fn emit_queue_lifecycle_event(
    pool: &PgPool,
    publisher: &Publisher,
    trigger_ref: &str,
    queue: &WorkQueue,
    payload: JsonValue,
) -> Result<Option<Event>> {
    let trigger_id: Option<Id> = sqlx::query_scalar("SELECT id FROM trigger WHERE ref = $1")
        .bind(trigger_ref)
        .fetch_optional(pool)
        .await?;

    let Some(trigger_id) = trigger_id else {
        warn!(
            "Skipping work queue lifecycle event '{}' for queue '{}' because trigger is not registered",
            trigger_ref, queue.r#ref
        );
        return Ok(None);
    };

    let event = EventRepository::create(
        pool,
        CreateEventInput {
            trigger: Some(trigger_id),
            trigger_ref: trigger_ref.to_string(),
            config: None,
            payload: Some(payload),
            trace_tag: None,
            source: None,
            source_ref: Some("attune.work_queue".to_string()),
            rule: None,
            rule_ref: None,
        },
    )
    .await?;

    let mq_payload = EventCreatedPayload {
        event_id: event.id,
        trigger_id: event.trigger,
        trigger_ref: event.trigger_ref.clone(),
        sensor_id: event.source,
        sensor_ref: event.source_ref.clone(),
        payload: event.payload.clone().unwrap_or_else(|| json!({})),
        config: event.config.clone(),
    };
    let envelope =
        MessageEnvelope::new(MessageType::EventCreated, mq_payload).with_source("work-queue");
    publisher.publish_envelope(&envelope).await?;

    Ok(Some(event))
}
