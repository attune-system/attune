//! Integration tests for Event repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Event repository.
//! Note: Events are immutable time-series data — there are no update tests.

mod helpers;

use attune_common::{
    repositories::{
        event::{CreateEventInput, EventRepository},
        Create, Delete, FindById, List,
    },
    Error,
};
use helpers::*;
use serde_json::json;

// ============================================================================
// CREATE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_event_minimal() {
    let pool = create_test_pool().await.unwrap();

    // Create a trigger for the event
    let pack = PackFixture::new_unique("event_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    // Create event with minimal fields
    let input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: trigger.r#ref.clone(),
        config: None,
        payload: None,
        source: None,
        source_ref: None,
        rule: None,
        rule_ref: None,
    };

    let event = EventRepository::create(&pool, input).await.unwrap();

    assert!(event.id > 0);
    assert_eq!(event.trigger, Some(trigger.id));
    assert_eq!(event.trigger_ref, trigger.r#ref);
    assert_eq!(event.config, None);
    assert_eq!(event.payload, None);
    assert_eq!(event.source, None);
    assert_eq!(event.source_ref, None);
    assert!(event.created.timestamp() > 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_event_with_payload() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("payload_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let payload = json!({
        "webhook_url": "https://example.com/webhook",
        "method": "POST",
        "headers": {
            "Content-Type": "application/json"
        },
        "body": {
            "message": "Test event"
        }
    });

    let input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: trigger.r#ref.clone(),
        config: None,
        payload: Some(payload.clone()),
        source: None,
        source_ref: None,
        rule: None,
        rule_ref: None,
    };

    let event = EventRepository::create(&pool, input).await.unwrap();

    assert_eq!(event.payload, Some(payload));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_event_with_config() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("config_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "timer")
        .create(&pool)
        .await
        .unwrap();

    let config = json!({
        "interval": "5m",
        "timezone": "UTC"
    });

    let input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: trigger.r#ref.clone(),
        config: Some(config.clone()),
        payload: None,
        source: None,
        source_ref: None,
        rule: None,
        rule_ref: None,
    };

    let event = EventRepository::create(&pool, input).await.unwrap();

    assert_eq!(event.config, Some(config));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_event_without_trigger_id() {
    let pool = create_test_pool().await.unwrap();

    // Events can be created without a trigger ID (trigger may have been deleted)
    let input = CreateEventInput {
        trigger: None,
        trigger_ref: "deleted.trigger".to_string(),
        config: None,
        payload: Some(json!({"reason": "trigger was deleted"})),
        source: None,
        source_ref: None,
        rule: None,
        rule_ref: None,
    };

    let event = EventRepository::create(&pool, input).await.unwrap();

    assert_eq!(event.trigger, None);
    assert_eq!(event.trigger_ref, "deleted.trigger");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_event_with_source() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("source_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    // Create a sensor to reference as source
    // Note: We'd need a SensorFixture, but for now we'll just test with NULL source
    let input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: trigger.r#ref.clone(),
        config: None,
        payload: None,
        source: None,
        source_ref: Some("test.sensor".to_string()),
        rule: None,
        rule_ref: None,
    };

    let event = EventRepository::create(&pool, input).await.unwrap();

    assert_eq!(event.source, None);
    assert_eq!(event.source_ref, Some("test.sensor".to_string()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_event_with_invalid_trigger_fails() {
    let pool = create_test_pool().await.unwrap();

    // Try to create event with non-existent trigger ID
    let input = CreateEventInput {
        trigger: Some(99999),
        trigger_ref: "nonexistent.trigger".to_string(),
        config: None,
        payload: None,
        source: None,
        source_ref: None,
        rule: None,
        rule_ref: None,
    };

    let result = EventRepository::create(&pool, input).await;

    assert!(result.is_err());
    // Foreign key constraint violation
}

// ============================================================================
// READ Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_event_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("find_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let created_event = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
        .with_payload(json!({"test": "data"}))
        .create(&pool)
        .await
        .unwrap();

    let found = EventRepository::find_by_id(&pool, created_event.id)
        .await
        .unwrap();

    assert!(found.is_some());
    let event = found.unwrap();
    assert_eq!(event.id, created_event.id);
    assert_eq!(event.trigger, created_event.trigger);
    assert_eq!(event.trigger_ref, created_event.trigger_ref);
    assert_eq!(event.payload, created_event.payload);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_event_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = EventRepository::find_by_id(&pool, 99999).await.unwrap();

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_event_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("get_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let created_event = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    let event = EventRepository::get_by_id(&pool, created_event.id)
        .await
        .unwrap();

    assert_eq!(event.id, created_event.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_event_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = EventRepository::get_by_id(&pool, 99999).await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::NotFound { .. }));
}

// ============================================================================
// LIST Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_events_empty() {
    let pool = create_test_pool().await.unwrap();

    let events = EventRepository::list(&pool).await.unwrap();
    // May have events from other tests, just verify we can list without error
    drop(events);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_events() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("list_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let before_count = EventRepository::list(&pool).await.unwrap().len();

    // Create multiple events
    let mut created_ids = vec![];
    for i in 0..3 {
        let event = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
            .with_payload(json!({"index": i}))
            .create(&pool)
            .await
            .unwrap();
        created_ids.push(event.id);
    }

    let events = EventRepository::list(&pool).await.unwrap();

    assert!(events.len() >= before_count + 3);
    // Verify our events are in the list (should be at the top since ordered by created DESC)
    let our_events: Vec<_> = events
        .iter()
        .filter(|e| created_ids.contains(&e.id))
        .collect();
    assert_eq!(our_events.len(), 3);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_events_respects_limit() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("limit_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    // List operation has a LIMIT of 1000, so it won't retrieve more than that
    let events = EventRepository::list(&pool).await.unwrap();
    assert!(events.len() <= 1000);
}

// ============================================================================
// DELETE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_event() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("delete_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let event = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    let deleted = EventRepository::delete(&pool, event.id).await.unwrap();

    assert!(deleted);

    // Verify it's gone
    let found = EventRepository::find_by_id(&pool, event.id).await.unwrap();
    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_event_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = EventRepository::delete(&pool, 99999).await.unwrap();

    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_event_enforcement_retains_event_id() {
    let pool = create_test_pool().await.unwrap();

    // Create pack, trigger, action, rule, and event
    let pack = PackFixture::new_unique("cascade_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    // Create a rule
    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let event = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    // Create enforcement referencing the event
    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .with_event(event.id)
        .create(&pool)
        .await
        .unwrap();

    // Delete the event — since the event table is a TimescaleDB hypertable, the FK
    // constraint from enforcement.event was dropped (hypertables cannot be FK targets).
    // The enforcement.event column retains the old ID as a dangling reference.
    EventRepository::delete(&pool, event.id).await.unwrap();

    // Enforcement still exists with the original event ID (now a dangling reference)
    use attune_common::repositories::event::EnforcementRepository;
    let found_enforcement = EnforcementRepository::find_by_id(&pool, enforcement.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found_enforcement.event, Some(event.id));
}

// ============================================================================
// SPECIALIZED QUERY Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_events_by_trigger() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("trigger_query_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger1 = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let trigger2 = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "timer")
        .create(&pool)
        .await
        .unwrap();

    // Create events for trigger1
    for i in 0..3 {
        EventFixture::new_unique(Some(trigger1.id), &trigger1.r#ref)
            .with_payload(json!({"trigger": 1, "index": i}))
            .create(&pool)
            .await
            .unwrap();
    }

    // Create events for trigger2
    for i in 0..2 {
        EventFixture::new_unique(Some(trigger2.id), &trigger2.r#ref)
            .with_payload(json!({"trigger": 2, "index": i}))
            .create(&pool)
            .await
            .unwrap();
    }

    let events = EventRepository::find_by_trigger(&pool, trigger1.id)
        .await
        .unwrap();

    assert_eq!(events.len(), 3);
    for event in &events {
        assert_eq!(event.trigger, Some(trigger1.id));
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_events_by_trigger_ref() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("triggerref_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    // Create events with a unique trigger_ref to avoid conflicts
    let unique_trigger_ref = trigger.r#ref.clone();
    for i in 0..3 {
        EventFixture::new(Some(trigger.id), &unique_trigger_ref)
            .with_payload(json!({"index": i}))
            .create(&pool)
            .await
            .unwrap();
    }

    let events = EventRepository::find_by_trigger_ref(&pool, &unique_trigger_ref)
        .await
        .unwrap();

    assert_eq!(events.len(), 3);
    for event in &events {
        assert_eq!(event.trigger_ref, unique_trigger_ref);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_events_by_trigger_ref_preserves_after_trigger_deletion() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("preserve_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let trigger_ref = trigger.r#ref.clone();

    // Create event with the specific trigger_ref
    let event = EventFixture::new(Some(trigger.id), &trigger_ref)
        .create(&pool)
        .await
        .unwrap();

    // Delete the trigger (ON DELETE SET NULL on event.trigger)
    use attune_common::repositories::{trigger::TriggerRepository, Delete};
    TriggerRepository::delete(&pool, trigger.id).await.unwrap();

    // Events should still be findable by trigger_ref even though trigger is deleted
    let events = EventRepository::find_by_trigger_ref(&pool, &trigger_ref)
        .await
        .unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, event.id);
    assert_eq!(events[0].trigger, None); // trigger ID set to NULL
    assert_eq!(events[0].trigger_ref, trigger_ref); // trigger_ref preserved
}

// ============================================================================
// TIMESTAMP Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_event_created_timestamp_auto_set() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("timestamp_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let event = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    assert!(event.created.timestamp() > 0);
}
