//! Integration tests for work queue repositories.

use attune_common::{
    models::{
        ActionReferenceVisibility, WorkQueueBatchMode, WorkQueueDispatchStatus,
        WorkQueueItemStatus, WorkQueueUpdateStrategy,
    },
    repositories::{
        work_queue::{
            CreateWorkQueueDispatchInput, CreateWorkQueueInput, CreateWorkQueueItemInput,
            LeaseWorkQueueItemsInput, ReleaseWorkQueueLeaseInput, UpdateWorkQueueDispatchInput,
            UpdateWorkQueueInput, WorkQueueDispatchRepository, WorkQueueDispatchSearchFilters,
            WorkQueueItemRepository, WorkQueueItemSearchFilters, WorkQueueRepository,
        },
        Create, Delete, FindByRef, Update,
    },
};
use chrono::{Duration, Utc};
use serde_json::json;
use uuid::Uuid;

mod helpers;
use helpers::{create_test_pool, ActionFixture, PackFixture};

async fn create_queue_fixture() -> (sqlx::PgPool, attune_common::models::work_queue::WorkQueue) {
    let pool = create_test_pool().await.expect("test pool");
    let pack = PackFixture::new_unique("queue_repo")
        .create(&pool)
        .await
        .expect("pack");
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "dispatch")
        .create(&pool)
        .await
        .expect("action");

    let queue = WorkQueueRepository::create(
        &pool,
        CreateWorkQueueInput {
            r#ref: format!("{}.inbox", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Inbox".to_string(),
            description: Some("Repository test queue".to_string()),
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 3,
            allow_pending_update: true,
            update_strategy: WorkQueueUpdateStrategy::MergePatch,
            batch_mode: WorkQueueBatchMode::Batch,
            item_schema: json!({
                "order_id": { "type": "integer", "required": true }
            }),
            action_params: json!({
                "items": "{{ items }}",
                "queue": "{{ queue }}"
            }),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({
                "dispatch": {
                    "batch_size": { "source": "literal", "value": 2 }
                },
                "ack_contract": { "version": 2 }
            }),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .expect("queue");

    (pool, queue)
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn work_queue_repository_crud_and_search_round_trip() {
    let (pool, queue) = create_queue_fixture().await;

    let found = WorkQueueRepository::find_by_ref(&pool, &queue.r#ref)
        .await
        .expect("find queue")
        .expect("queue should exist");
    assert_eq!(found.id, queue.id);
    assert_eq!(found.dispatch_action_ref, queue.dispatch_action_ref);
    assert!(found.accepting_new_items);
    assert_eq!(found.item_schema["order_id"]["type"], "integer");
    assert_eq!(found.action_params["items"], "{{ items }}");

    let search = WorkQueueRepository::search(
        &pool,
        &attune_common::repositories::work_queue::WorkQueueSearchFilters {
            pack_ref: queue.pack_ref.clone(),
            enabled: Some(true),
            limit: 10,
            offset: 0,
            ..Default::default()
        },
    )
    .await
    .expect("search queues");
    assert_eq!(search.total, 1);
    assert_eq!(search.rows[0].r#ref, queue.r#ref);

    let updated = WorkQueueRepository::update(
        &pool,
        queue.id,
        UpdateWorkQueueInput {
            label: Some("Inbox Updated".to_string()),
            enabled: Some(false),
            accepting_new_items: Some(false),
            default_priority: Some(7),
            item_schema: Some(json!({
                "order_id": { "type": "string" }
            })),
            action_params: Some(json!({
                "batch": "{{ items }}"
            })),
            ..Default::default()
        },
    )
    .await
    .expect("update queue");
    assert_eq!(updated.label, "Inbox Updated");
    assert!(!updated.enabled);
    assert!(!updated.accepting_new_items);
    assert_eq!(updated.default_priority, 7);
    assert_eq!(updated.item_schema["order_id"]["type"], "string");
    assert_eq!(updated.action_params["batch"], "{{ items }}");

    let deleted = WorkQueueRepository::delete(&pool, queue.id)
        .await
        .expect("delete queue");
    assert!(deleted);
    assert!(WorkQueueRepository::find_by_ref(&pool, &queue.r#ref)
        .await
        .expect("find deleted queue")
        .is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn work_queue_item_repository_leases_releases_and_reclaims_items() {
    let (pool, queue) = create_queue_fixture().await;

    let high = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("high".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"id": "high"}),
            metadata: json!({"source": "api"}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("high item");
    let retry = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("retry".to_string()),
            priority: 25,
            status: WorkQueueItemStatus::Retry,
            payload: json!({"id": "retry"}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 1,
            last_error: Some(json!({"message": "try again"})),
            ack_summary: None,
        },
    )
    .await
    .expect("retry item");
    let low = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("low".to_string()),
            priority: 1,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"id": "low"}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("low item");

    let lease_token = Uuid::new_v4();
    let lease_expires_at = Utc::now() + Duration::minutes(5);
    let leased = WorkQueueItemRepository::lease_next_batch(
        &pool,
        LeaseWorkQueueItemsInput {
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued, WorkQueueItemStatus::Retry],
            limit: 2,
            batch_coalescing: None,
            leased_execution: None,
            lease_token,
            lease_expires_at,
        },
    )
    .await
    .expect("lease items");

    assert_eq!(leased.len(), 2);
    assert_eq!(leased[0].id, high.id);
    assert_eq!(leased[1].id, retry.id);
    assert!(leased
        .iter()
        .all(|item| item.status == WorkQueueItemStatus::Leased));
    assert_eq!(leased[0].attempt_count, 1);
    assert_eq!(leased[1].attempt_count, 2);

    let attached = WorkQueueItemRepository::attach_execution_to_lease(&pool, lease_token, 4242)
        .await
        .expect("attach execution");
    assert_eq!(attached, 2);

    let released = WorkQueueItemRepository::release_lease(
        &pool,
        ReleaseWorkQueueLeaseInput {
            lease_token,
            new_status: WorkQueueItemStatus::Completed,
            leased_execution: None,
            last_error: None,
            ack_summary: Some(json!({"status": "completed"})),
        },
    )
    .await
    .expect("release lease");
    assert_eq!(released.len(), 2);
    assert!(released
        .iter()
        .all(|item| item.status == WorkQueueItemStatus::Completed));
    assert!(released.iter().all(|item| item.lease_token.is_none()));
    assert!(released.iter().all(|item| item.lease_expires_at.is_none()));
    assert_eq!(
        released[0]
            .ack_summary
            .as_ref()
            .and_then(|summary| summary.get("status"))
            .and_then(|value| value.as_str()),
        Some("completed")
    );

    let expired_token = Uuid::new_v4();
    let _expired = WorkQueueItemRepository::lease_next_batch(
        &pool,
        LeaseWorkQueueItemsInput {
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: 1,
            batch_coalescing: None,
            leased_execution: Some(5150),
            lease_token: expired_token,
            lease_expires_at: Utc::now() - Duration::minutes(1),
        },
    )
    .await
    .expect("lease low item");

    let reclaimed = WorkQueueItemRepository::reclaim_expired_leases(
        &pool,
        Utc::now(),
        Some(queue.id),
        WorkQueueItemStatus::Retry,
    )
    .await
    .expect("reclaim expired lease");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].id, low.id);
    assert_eq!(reclaimed[0].status, WorkQueueItemStatus::Retry);
    assert!(reclaimed[0].lease_token.is_none());

    let retry_rows = WorkQueueItemRepository::search(
        &pool,
        &WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            statuses: Some(vec![WorkQueueItemStatus::Retry]),
            limit: 10,
            offset: 0,
            ..Default::default()
        },
    )
    .await
    .expect("search retry rows");
    assert_eq!(retry_rows.total, 1);
    assert_eq!(retry_rows.rows[0].id, low.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn work_queue_item_repository_coalesces_within_same_priority_band() {
    let (pool, queue) = create_queue_fixture().await;

    let group_member_1 = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("gm-1".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "GroupMember"}, "id": 1}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("first group member");
    let _user = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("user-1".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "User"}, "id": 2}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("user");
    let group_member_2 = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("gm-2".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "GroupMember"}, "id": 3}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("second group member");
    let _lower_priority_group_member = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("gm-3".to_string()),
            priority: 40,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "GroupMember"}, "id": 4}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("lower priority group member");

    let leased = WorkQueueItemRepository::lease_next_batch(
        &pool,
        LeaseWorkQueueItemsInput {
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: 3,
            batch_coalescing: Some(attune_common::models::WorkQueueBatchCoalescingConfig {
                enabled: true,
                group_by_path: Some("attributes.sobject_type".to_string()),
                across_priorities: false,
            }),
            leased_execution: None,
            lease_token: Uuid::new_v4(),
            lease_expires_at: Utc::now() + Duration::minutes(5),
        },
    )
    .await
    .expect("lease coalesced batch");

    assert_eq!(leased.len(), 2);
    assert_eq!(leased[0].id, group_member_1.id);
    assert_eq!(leased[1].id, group_member_2.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn work_queue_item_repository_can_coalesce_across_priorities() {
    let (pool, queue) = create_queue_fixture().await;

    let group_member_1 = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("gm-1".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "GroupMember"}, "id": 1}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("first group member");
    let _user = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("user-1".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "User"}, "id": 2}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("user");
    let group_member_2 = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("gm-2".to_string()),
            priority: 40,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "GroupMember"}, "id": 3}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("second group member");
    let group_member_3 = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("gm-3".to_string()),
            priority: 30,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "GroupMember"}, "id": 4}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("third group member");

    let leased = WorkQueueItemRepository::lease_next_batch(
        &pool,
        LeaseWorkQueueItemsInput {
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: 3,
            batch_coalescing: Some(attune_common::models::WorkQueueBatchCoalescingConfig {
                enabled: true,
                group_by_path: Some("attributes.sobject_type".to_string()),
                across_priorities: true,
            }),
            leased_execution: None,
            lease_token: Uuid::new_v4(),
            lease_expires_at: Utc::now() + Duration::minutes(5),
        },
    )
    .await
    .expect("lease coalesced batch");

    assert_eq!(leased.len(), 3);
    assert_eq!(leased[0].id, group_member_1.id);
    assert_eq!(leased[1].id, group_member_2.id);
    assert_eq!(leased[2].id, group_member_3.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn work_queue_item_repository_returns_partial_batch_when_only_partial_group_matches() {
    let (pool, queue) = create_queue_fixture().await;

    let group_member = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("gm-1".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "GroupMember"}, "id": 1}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("group member");
    let _user = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("user-1".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"attributes": {"sobject_type": "User"}, "id": 2}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("user");

    let leased = WorkQueueItemRepository::lease_next_batch(
        &pool,
        LeaseWorkQueueItemsInput {
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: 3,
            batch_coalescing: Some(attune_common::models::WorkQueueBatchCoalescingConfig {
                enabled: true,
                group_by_path: Some("attributes.sobject_type".to_string()),
                across_priorities: false,
            }),
            leased_execution: None,
            lease_token: Uuid::new_v4(),
            lease_expires_at: Utc::now() + Duration::minutes(5),
        },
    )
    .await
    .expect("lease partial coalesced batch");

    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].id, group_member.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn work_queue_item_repository_falls_back_to_fifo_when_group_value_is_missing() {
    let (pool, queue) = create_queue_fixture().await;

    let first = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("first".to_string()),
            priority: 50,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"id": 1}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("first item");
    let second = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: Some("second".to_string()),
            priority: 40,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"id": 2, "attributes": {"sobject_type": "User"}}),
            metadata: json!({}),
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("second item");

    let leased = WorkQueueItemRepository::lease_next_batch(
        &pool,
        LeaseWorkQueueItemsInput {
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: 2,
            batch_coalescing: Some(attune_common::models::WorkQueueBatchCoalescingConfig {
                enabled: true,
                group_by_path: Some("attributes.sobject_type".to_string()),
                across_priorities: true,
            }),
            leased_execution: None,
            lease_token: Uuid::new_v4(),
            lease_expires_at: Utc::now() + Duration::minutes(5),
        },
    )
    .await
    .expect("lease fallback batch");

    assert_eq!(leased.len(), 2);
    assert_eq!(leased[0].id, first.id);
    assert_eq!(leased[1].id, second.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn work_queue_dispatch_repository_tracks_active_and_terminal_dispatches() {
    let (pool, queue) = create_queue_fixture().await;

    let dispatch = WorkQueueDispatchRepository::create(
        &pool,
        CreateWorkQueueDispatchInput {
            id: None,
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            execution: 9001,
            status: WorkQueueDispatchStatus::Leased,
            leased_item_count: 2,
        },
    )
    .await
    .expect("create dispatch");

    let found = WorkQueueDispatchRepository::find_by_execution(&pool, dispatch.execution)
        .await
        .expect("find dispatch")
        .expect("dispatch exists");
    assert_eq!(found.id, dispatch.id);

    let active = WorkQueueDispatchRepository::list_active(&pool)
        .await
        .expect("list active");
    assert!(active.iter().any(|row| row.id == dispatch.id));

    let filtered = WorkQueueDispatchRepository::search(
        &pool,
        &WorkQueueDispatchSearchFilters {
            queue: Some(queue.id),
            statuses: Some(vec![WorkQueueDispatchStatus::Leased]),
            limit: 10,
            offset: 0,
            ..Default::default()
        },
    )
    .await
    .expect("search dispatches");
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.rows[0].id, dispatch.id);

    let updated = WorkQueueDispatchRepository::update(
        &pool,
        dispatch.id,
        UpdateWorkQueueDispatchInput {
            status: Some(WorkQueueDispatchStatus::Completed),
            leased_item_count: Some(2),
        },
    )
    .await
    .expect("update dispatch");
    assert_eq!(updated.status, WorkQueueDispatchStatus::Completed);

    let active_after = WorkQueueDispatchRepository::list_active(&pool)
        .await
        .expect("list active after completion");
    assert!(!active_after.iter().any(|row| row.id == dispatch.id));
}
