mod helpers;

use serde_json::Value;
use sqlx::{postgres::PgListener, Row};
use std::time::Duration;
use tokio::time::timeout;

async fn receive_payload(
    listener: &mut PgListener,
    channel: &str,
    discriminator: &str,
    expected: &str,
) -> Value {
    timeout(Duration::from_secs(5), async {
        loop {
            let notification = listener.recv().await.expect("receive notification");
            let payload: Value =
                serde_json::from_str(notification.payload()).expect("parse notification payload");
            if notification.channel() == channel && payload[discriminator] == expected {
                return payload;
            }
        }
    })
    .await
    .expect("notification timeout")
}

#[tokio::test]
async fn related_entity_notifications_keep_routing_and_omit_business_identifiers() {
    let pool = helpers::create_test_pool().await.expect("test pool");
    let mut listener = PgListener::connect_with(&pool)
        .await
        .expect("connect listener");
    listener
        .listen_all([
            "event_created",
            "enforcement_created",
            "enforcement_status_changed",
            "work_queue_item_created",
            "work_queue_item_updated",
            "artifact_created",
            "artifact_updated",
        ])
        .await
        .expect("listen for notifications");

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let trigger_ref = format!("test.trigger-{suffix}");
    let source_trace_tag = format!("trace.source.{suffix}");
    let event = sqlx::query(
        r#"
        INSERT INTO event (trigger_ref, payload, trace_tag)
        VALUES ($1, '{}'::jsonb, $2)
        RETURNING id
        "#,
    )
    .bind(&trigger_ref)
    .bind(&source_trace_tag)
    .fetch_one(&pool)
    .await
    .expect("create event");
    let event_id: i64 = event.get("id");

    let event_payload =
        receive_payload(&mut listener, "event_created", "trigger_ref", &trigger_ref).await;
    assert_eq!(event_payload["entity_id"], event_id);
    assert!(event_payload.get("trace_tag").is_none());

    let rule_ref = format!("test.rule-{suffix}");
    let enforcement = sqlx::query(
        r#"
        INSERT INTO enforcement (rule_ref, trigger_ref, event, payload)
        VALUES ($1, $2, $3, '{}'::jsonb)
        RETURNING id
        "#,
    )
    .bind(&rule_ref)
    .bind(&trigger_ref)
    .bind(event_id)
    .fetch_one(&pool)
    .await
    .expect("create enforcement");
    let enforcement_id: i64 = enforcement.get("id");

    let enforcement_created =
        receive_payload(&mut listener, "enforcement_created", "rule_ref", &rule_ref).await;
    assert_eq!(enforcement_created["entity_id"], enforcement_id);
    assert!(enforcement_created.get("trace_tag").is_none());

    let execution_trace_tag = format!("trace.execution.{suffix}");
    sqlx::query(
        r#"
        INSERT INTO execution (action_ref, enforcement, trace_tag)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(format!("test.action-{suffix}"))
    .bind(enforcement_id)
    .bind(&execution_trace_tag)
    .execute(&pool)
    .await
    .expect("create linked execution");
    sqlx::query("UPDATE enforcement SET status = 'processed' WHERE id = $1")
        .bind(enforcement_id)
        .execute(&pool)
        .await
        .expect("update enforcement");

    let enforcement_updated = receive_payload(
        &mut listener,
        "enforcement_status_changed",
        "rule_ref",
        &rule_ref,
    )
    .await;
    assert_eq!(enforcement_updated["entity_id"], enforcement_id);
    assert!(enforcement_updated.get("trace_tag").is_none());

    let queue_ref = format!("test.queue-{suffix}");
    let queue = sqlx::query(
        r#"
        INSERT INTO work_queue (ref, label, dispatch_action_ref)
        VALUES ($1, 'Test queue', $2)
        RETURNING id
        "#,
    )
    .bind(&queue_ref)
    .bind(format!("test.action-{suffix}"))
    .fetch_one(&pool)
    .await
    .expect("create work queue");
    let queue_id: i64 = queue.get("id");
    let queue_trace_tag = format!("trace.queue.{suffix}");
    let item = sqlx::query(
        r#"
        INSERT INTO work_queue_item (queue, queue_ref, payload, trace_tag, enqueue_source, item_key)
        VALUES ($1, $2, '{}'::jsonb, $3, 'test', 'fixture-external-reference')
        RETURNING id
        "#,
    )
    .bind(queue_id)
    .bind(&queue_ref)
    .bind(&queue_trace_tag)
    .fetch_one(&pool)
    .await
    .expect("create work queue item");
    let item_id: i64 = item.get("id");

    let item_created = receive_payload(
        &mut listener,
        "work_queue_item_created",
        "queue_ref",
        &queue_ref,
    )
    .await;
    assert_eq!(item_created["entity_id"], item_id);
    assert!(item_created.get("trace_tag").is_none());
    assert!(item_created.get("item_key").is_none());

    sqlx::query("UPDATE work_queue_item SET status = 'completed' WHERE id = $1")
        .bind(item_id)
        .execute(&pool)
        .await
        .expect("update work queue item");
    let item_updated = receive_payload(
        &mut listener,
        "work_queue_item_updated",
        "queue_ref",
        &queue_ref,
    )
    .await;
    assert_eq!(item_updated["entity_id"], item_id);
    assert!(item_updated.get("trace_tag").is_none());
    assert!(item_updated.get("item_key").is_none());
    assert_eq!(item_updated["status"], "completed");

    let compact_trigger_ref = format!("test.compact-trigger-{suffix}");
    let compact_trace_tag = format!("trace.compact.{suffix}");
    sqlx::query(
        r#"
        INSERT INTO event (trigger_ref, source_ref, payload, trace_tag)
        VALUES ($1, $2, '{}'::jsonb, $3)
        "#,
    )
    .bind(&compact_trigger_ref)
    .bind("x".repeat(7100))
    .bind(&compact_trace_tag)
    .execute(&pool)
    .await
    .expect("create event with oversized full notification");

    let compact_event = receive_payload(
        &mut listener,
        "event_created",
        "trigger_ref",
        &compact_trigger_ref,
    )
    .await;
    assert_eq!(compact_event["auth_mode"], "full");
    assert!(compact_event.get("trace_tag").is_none());
    assert!(compact_event.get("source_ref").is_none());

    let artifact_ref = format!("test.progress-{suffix}");
    let artifact: i64 = sqlx::query_scalar(
        "INSERT INTO artifact (ref, type, visibility, name)
         VALUES ($1, 'progress', 'private', 'private fixture name') RETURNING id",
    )
    .bind(&artifact_ref)
    .fetch_one(&pool)
    .await
    .expect("create private progress artifact");
    let created = receive_payload(&mut listener, "artifact_created", "ref", &artifact_ref).await;
    assert_eq!(created["entity_id"], artifact);
    assert!(created.get("name").is_none());

    sqlx::query("UPDATE artifact SET data = $2 WHERE id = $1")
        .bind(artifact)
        .bind(serde_json::json!([{"percent": 42, "message": "private fixture progress"}]))
        .execute(&pool)
        .await
        .expect("update private progress artifact");
    let updated = receive_payload(&mut listener, "artifact_updated", "ref", &artifact_ref).await;
    assert_eq!(updated["progress_percent"], 42.0);
    assert_eq!(updated["progress_entries"], 1);
    assert!(updated.get("progress_message").is_none());
}
