//! Integration tests for the Notification repository

use attune_common::{
    models::{enums::NotificationState, notification::Notification, JsonDict},
    repositories::{
        notification::{CreateNotificationInput, NotificationRepository, UpdateNotificationInput},
        Create, Delete, FindById, List, Update,
    },
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};

mod helpers;
use helpers::{create_test_pool, set_created_for_test, set_updated_for_test};

static NOTIFICATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Test fixture for creating unique notifications
struct NotificationFixture {
    pool: PgPool,
    id_suffix: u64,
}

impl NotificationFixture {
    fn new(pool: PgPool) -> Self {
        let id_suffix = NOTIFICATION_COUNTER.fetch_add(1, Ordering::SeqCst);
        Self { pool, id_suffix }
    }

    fn unique_channel(&self, base: &str) -> String {
        format!("{}_{}", base, self.id_suffix)
    }

    fn unique_entity(&self, base: &str) -> String {
        format!("{}_{}", base, self.id_suffix)
    }

    async fn create_notification(
        &self,
        channel: &str,
        entity_type: &str,
        entity: &str,
        activity: &str,
        state: NotificationState,
        content: Option<JsonDict>,
    ) -> Notification {
        let input = CreateNotificationInput {
            channel: channel.to_string(),
            entity_type: entity_type.to_string(),
            entity: entity.to_string(),
            activity: activity.to_string(),
            state,
            content,
        };

        NotificationRepository::create(&self.pool, input)
            .await
            .expect("Failed to create notification")
    }

    async fn create_default(&self) -> Notification {
        let channel = self.unique_channel("test_channel");
        let entity = self.unique_entity("test_entity");
        self.create_notification(
            &channel,
            "execution",
            &entity,
            "created",
            NotificationState::Created,
            None,
        )
        .await
    }

    async fn create_with_content(&self, content: JsonDict) -> Notification {
        let channel = self.unique_channel("test_channel");
        let entity = self.unique_entity("test_entity");
        self.create_notification(
            &channel,
            "execution",
            &entity,
            "created",
            NotificationState::Created,
            Some(content),
        )
        .await
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_notification_minimal() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel = fixture.unique_channel("test_channel");
    let entity = fixture.unique_entity("entity_123");

    let input = CreateNotificationInput {
        channel: channel.clone(),
        entity_type: "execution".to_string(),
        entity: entity.clone(),
        activity: "created".to_string(),
        state: NotificationState::Created,
        content: None,
    };

    let notification = NotificationRepository::create(&pool, input)
        .await
        .expect("Failed to create notification");

    assert!(notification.id > 0);
    assert_eq!(notification.channel, channel);
    assert_eq!(notification.entity_type, "execution");
    assert_eq!(notification.entity, entity);
    assert_eq!(notification.activity, "created");
    assert_eq!(notification.state, NotificationState::Created);
    assert!(notification.content.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_notification_with_content() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel = fixture.unique_channel("test_channel");
    let entity = fixture.unique_entity("entity_456");
    let content = json!({
        "execution_id": 123,
        "status": "running",
        "progress": 50
    });

    let input = CreateNotificationInput {
        channel: channel.clone(),
        entity_type: "execution".to_string(),
        entity: entity.clone(),
        activity: "updated".to_string(),
        state: NotificationState::Queued,
        content: Some(content.clone()),
    };

    let notification = NotificationRepository::create(&pool, input)
        .await
        .expect("Failed to create notification");

    assert!(notification.id > 0);
    assert_eq!(notification.channel, channel);
    assert_eq!(notification.state, NotificationState::Queued);
    assert!(notification.content.is_some());
    assert_eq!(notification.content.unwrap(), content);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_notification_all_states() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let states = [
        NotificationState::Created,
        NotificationState::Queued,
        NotificationState::Processing,
        NotificationState::Error,
    ];

    for state in states {
        let channel = fixture.unique_channel(&format!("channel_{:?}", state));
        let entity = fixture.unique_entity(&format!("entity_{:?}", state));

        let input = CreateNotificationInput {
            channel,
            entity_type: "test".to_string(),
            entity,
            activity: "test".to_string(),
            state,
            content: None,
        };

        let notification = NotificationRepository::create(&pool, input)
            .await
            .expect("Failed to create notification");

        assert_eq!(notification.state, state);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_notification_by_id() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let found = NotificationRepository::find_by_id(&pool, created.id)
        .await
        .expect("Failed to find notification")
        .expect("Notification not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.channel, created.channel);
    assert_eq!(found.entity_type, created.entity_type);
    assert_eq!(found.entity, created.entity);
    assert_eq!(found.activity, created.activity);
    assert_eq!(found.state, created.state);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_notification_by_id_not_found() {
    let pool = create_test_pool().await.expect("Failed to create pool");

    let result = NotificationRepository::find_by_id(&pool, 999_999_999)
        .await
        .expect("Query should succeed");

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_notification_state() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let created = fixture.create_default().await;
    assert_eq!(created.state, NotificationState::Created);

    let update_input = UpdateNotificationInput {
        state: Some(NotificationState::Processing),
        content: None,
    };

    let updated = NotificationRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update notification");

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.state, NotificationState::Processing);
    assert_eq!(updated.channel, created.channel);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_notification_content() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let created = fixture.create_default().await;
    assert!(created.content.is_none());

    let new_content = json!({
        "error": "Something went wrong",
        "code": 500
    });

    let update_input = UpdateNotificationInput {
        state: None,
        content: Some(new_content.clone()),
    };

    let updated = NotificationRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update notification");

    assert_eq!(updated.id, created.id);
    assert!(updated.content.is_some());
    assert_eq!(updated.content.unwrap(), new_content);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_notification_state_and_content() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let new_content = json!({
        "message": "Processing complete"
    });

    let update_input = UpdateNotificationInput {
        state: Some(NotificationState::Processing),
        content: Some(new_content.clone()),
    };

    let updated = NotificationRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update notification");

    assert_eq!(updated.state, NotificationState::Processing);
    assert_eq!(updated.content.unwrap(), new_content);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_notification_no_changes() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let update_input = UpdateNotificationInput {
        state: None,
        content: None,
    };

    let updated = NotificationRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update notification");

    // Should return existing entity unchanged
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.state, created.state);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_notification_timestamps() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let created = fixture.create_default().await;
    let created_timestamp = created.created;
    let original_updated = created.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "notification", created.id, original_updated).await;

    let update_input = UpdateNotificationInput {
        state: Some(NotificationState::Queued),
        content: None,
    };

    let updated = NotificationRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update notification");

    // created should be unchanged
    assert_eq!(updated.created, created_timestamp);
    // updated should be newer
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_notification() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let deleted = NotificationRepository::delete(&pool, created.id)
        .await
        .expect("Failed to delete notification");

    assert!(deleted);

    let found = NotificationRepository::find_by_id(&pool, created.id)
        .await
        .expect("Query should succeed");

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_notification_not_found() {
    let pool = create_test_pool().await.expect("Failed to create pool");

    let deleted = NotificationRepository::delete(&pool, 999_999_999)
        .await
        .expect("Delete should succeed");

    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_notifications() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    // Create multiple notifications
    let n1 = fixture.create_default().await;
    let n2 = fixture.create_default().await;
    let n3 = fixture.create_default().await;
    set_created_for_test(
        &pool,
        "notification",
        &[n1.id, n2.id, n3.id],
        chrono::Utc::now() - chrono::Duration::seconds(1),
    )
    .await;

    let notifications = NotificationRepository::list(&pool)
        .await
        .expect("Failed to list notifications");

    // Should contain our created notifications
    let ids: Vec<i64> = notifications.iter().map(|n| n.id).collect();
    assert!(ids.contains(&n1.id));
    assert!(ids.contains(&n2.id));
    assert!(ids.contains(&n3.id));

    // Should be ordered by created DESC (newest first)
    let our_notifications: Vec<&Notification> = notifications
        .iter()
        .filter(|n| [n1.id, n2.id, n3.id].contains(&n.id))
        .collect();

    if our_notifications.len() >= 3 {
        // Find positions of our notifications
        let pos1 = notifications.iter().position(|n| n.id == n1.id).unwrap();
        let pos2 = notifications.iter().position(|n| n.id == n2.id).unwrap();
        let pos3 = notifications.iter().position(|n| n.id == n3.id).unwrap();

        // n3 (newest) should come before n2, which should come before n1 (oldest)
        assert!(pos3 < pos2);
        assert!(pos2 < pos1);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_state() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel1 = fixture.unique_channel("channel_1");
    let entity1 = fixture.unique_entity("entity_1");

    let channel2 = fixture.unique_channel("channel_2");
    let entity2 = fixture.unique_entity("entity_2");

    // Create notifications with different states
    let n1 = fixture
        .create_notification(
            &channel1,
            "execution",
            &entity1,
            "created",
            NotificationState::Queued,
            None,
        )
        .await;

    let n2 = fixture
        .create_notification(
            &channel2,
            "execution",
            &entity2,
            "created",
            NotificationState::Queued,
            None,
        )
        .await;

    let _n3 = fixture
        .create_notification(
            &fixture.unique_channel("channel_3"),
            "execution",
            &fixture.unique_entity("entity_3"),
            "created",
            NotificationState::Processing,
            None,
        )
        .await;

    let queued = NotificationRepository::find_by_state(&pool, NotificationState::Queued)
        .await
        .expect("Failed to find by state");

    let queued_ids: Vec<i64> = queued.iter().map(|n| n.id).collect();
    assert!(queued_ids.contains(&n1.id));
    assert!(queued_ids.contains(&n2.id));

    // All returned notifications should be Queued
    for notification in &queued {
        assert_eq!(notification.state, NotificationState::Queued);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_state_empty() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    // Create notification with Created state
    let _n = fixture.create_default().await;

    // Query for Error state (none should exist for our test data)
    let errors = NotificationRepository::find_by_state(&pool, NotificationState::Error)
        .await
        .expect("Failed to find by state");

    // Should not contain our notification
    // (might contain others from other tests, so just verify it works)
    assert!(errors.iter().all(|n| n.state == NotificationState::Error));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_channel() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel1 = fixture.unique_channel("channel_alpha");
    let channel2 = fixture.unique_channel("channel_beta");

    // Create notifications on different channels
    let n1 = fixture
        .create_notification(
            &channel1,
            "execution",
            &fixture.unique_entity("entity_1"),
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    let n2 = fixture
        .create_notification(
            &channel1,
            "execution",
            &fixture.unique_entity("entity_2"),
            "updated",
            NotificationState::Queued,
            None,
        )
        .await;

    let _n3 = fixture
        .create_notification(
            &channel2,
            "execution",
            &fixture.unique_entity("entity_3"),
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    let channel1_notifications = NotificationRepository::find_by_channel(&pool, &channel1)
        .await
        .expect("Failed to find by channel");

    let channel1_ids: Vec<i64> = channel1_notifications.iter().map(|n| n.id).collect();
    assert!(channel1_ids.contains(&n1.id));
    assert!(channel1_ids.contains(&n2.id));

    // All returned notifications should be on channel1
    for notification in &channel1_notifications {
        assert_eq!(notification.channel, channel1);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_channel_empty() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let nonexistent_channel = fixture.unique_channel("nonexistent_channel_xyz");

    let notifications = NotificationRepository::find_by_channel(&pool, &nonexistent_channel)
        .await
        .expect("Failed to find by channel");

    assert!(notifications.is_empty());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_with_complex_content() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let complex_content = json!({
        "execution_id": 123,
        "status": "completed",
        "result": {
            "stdout": "Command executed successfully",
            "stderr": "",
            "exit_code": 0
        },
        "metrics": {
            "duration_ms": 1500,
            "memory_mb": 128
        },
        "tags": ["production", "automated"]
    });

    let notification = fixture.create_with_content(complex_content.clone()).await;

    assert!(notification.content.is_some());
    assert_eq!(notification.content.unwrap(), complex_content);

    // Verify it's retrievable
    let found = NotificationRepository::find_by_id(&pool, notification.id)
        .await
        .expect("Failed to find notification")
        .expect("Notification not found");

    assert_eq!(found.content.unwrap(), complex_content);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_entity_types() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let entity_types = ["execution", "inquiry", "enforcement", "sensor", "action"];

    for entity_type in entity_types {
        let channel = fixture.unique_channel("test_channel");
        let entity = fixture.unique_entity(&format!("entity_{}", entity_type));

        let notification = fixture
            .create_notification(
                &channel,
                entity_type,
                &entity,
                "created",
                NotificationState::Created,
                None,
            )
            .await;

        assert_eq!(notification.entity_type, entity_type);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_activity_types() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let activities = ["created", "updated", "completed", "failed", "cancelled"];

    for activity in activities {
        let channel = fixture.unique_channel("test_channel");
        let entity = fixture.unique_entity(&format!("entity_{}", activity));

        let notification = fixture
            .create_notification(
                &channel,
                "execution",
                &entity,
                activity,
                NotificationState::Created,
                None,
            )
            .await;

        assert_eq!(notification.activity, activity);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_ordering_by_created() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel = fixture.unique_channel("ordered_channel");

    // Create notifications, then force a timestamp tie.
    let n1 = fixture
        .create_notification(
            &channel,
            "execution",
            &fixture.unique_entity("e1"),
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    let n2 = fixture
        .create_notification(
            &channel,
            "execution",
            &fixture.unique_entity("e2"),
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    let n3 = fixture
        .create_notification(
            &channel,
            "execution",
            &fixture.unique_entity("e3"),
            "created",
            NotificationState::Created,
            None,
        )
        .await;
    set_created_for_test(
        &pool,
        "notification",
        &[n1.id, n2.id, n3.id],
        chrono::Utc::now() - chrono::Duration::seconds(1),
    )
    .await;

    // Query by channel (should be ordered DESC by created)
    let notifications = NotificationRepository::find_by_channel(&pool, &channel)
        .await
        .expect("Failed to find by channel");

    let ids: Vec<i64> = notifications.iter().map(|n| n.id).collect();

    // Should be in reverse chronological order
    let pos1 = ids.iter().position(|&id| id == n1.id).unwrap();
    let pos2 = ids.iter().position(|&id| id == n2.id).unwrap();
    let pos3 = ids.iter().position(|&id| id == n3.id).unwrap();

    assert!(pos3 < pos2); // n3 (newest) before n2
    assert!(pos2 < pos1); // n2 before n1 (oldest)
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_timestamps_auto_set() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let before = chrono::Utc::now();
    let notification = fixture.create_default().await;
    let after = chrono::Utc::now();

    assert!(notification.created >= before);
    assert!(notification.created <= after);
    assert!(notification.updated >= before);
    assert!(notification.updated <= after);
    // Initially, created and updated should be very close
    assert!(
        (notification.updated - notification.created)
            .num_milliseconds()
            .abs()
            < 1000
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_multiple_notifications_same_entity() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel = fixture.unique_channel("execution_channel");
    let entity = fixture.unique_entity("execution_123");

    // Create multiple notifications for the same entity with different activities
    let n1 = fixture
        .create_notification(
            &channel,
            "execution",
            &entity,
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    let n2 = fixture
        .create_notification(
            &channel,
            "execution",
            &entity,
            "running",
            NotificationState::Processing,
            None,
        )
        .await;

    let n3 = fixture
        .create_notification(
            &channel,
            "execution",
            &entity,
            "completed",
            NotificationState::Processing,
            None,
        )
        .await;

    // All should exist with same entity but different activities
    assert_eq!(n1.entity, entity);
    assert_eq!(n2.entity, entity);
    assert_eq!(n3.entity, entity);

    assert_eq!(n1.activity, "created");
    assert_eq!(n2.activity, "running");
    assert_eq!(n3.activity, "completed");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_content_null_vs_empty_json() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    // Notification with no content
    let n1 = fixture.create_default().await;
    assert!(n1.content.is_none());

    // Notification with empty JSON object
    let n2 = fixture.create_with_content(json!({})).await;
    assert!(n2.content.is_some());
    assert_eq!(n2.content.as_ref().unwrap(), &json!({}));

    // They should be different
    assert_ne!(n1.content, n2.content);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_notification_content_to_null() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    // Create with content
    let notification = fixture.create_with_content(json!({"key": "value"})).await;
    assert!(notification.content.is_some());

    // Update content to explicit null (empty JSON object in this case)
    let update_input = UpdateNotificationInput {
        state: None,
        content: Some(json!(null)),
    };

    let updated = NotificationRepository::update(&pool, notification.id, update_input)
        .await
        .expect("Failed to update notification");

    assert!(updated.content.is_some());
    assert_eq!(updated.content.unwrap(), json!(null));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_state_transition_workflow() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    // Create notification in Created state
    let notification = fixture.create_default().await;
    assert_eq!(notification.state, NotificationState::Created);

    // Transition to Queued
    let n = NotificationRepository::update(
        &pool,
        notification.id,
        UpdateNotificationInput {
            state: Some(NotificationState::Queued),
            content: None,
        },
    )
    .await
    .expect("Failed to update");
    assert_eq!(n.state, NotificationState::Queued);

    // Transition to Processing
    let n = NotificationRepository::update(
        &pool,
        notification.id,
        UpdateNotificationInput {
            state: Some(NotificationState::Processing),
            content: None,
        },
    )
    .await
    .expect("Failed to update");
    assert_eq!(n.state, NotificationState::Processing);

    // Transition to Error
    let n = NotificationRepository::update(
        &pool,
        notification.id,
        UpdateNotificationInput {
            state: Some(NotificationState::Error),
            content: Some(json!({"error": "Failed to deliver"})),
        },
    )
    .await
    .expect("Failed to update");
    assert_eq!(n.state, NotificationState::Error);
    assert_eq!(n.content.unwrap(), json!({"error": "Failed to deliver"}));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_list_limit() {
    let pool = create_test_pool().await.expect("Failed to create pool");

    let notifications = NotificationRepository::list(&pool)
        .await
        .expect("Failed to list notifications");

    // List should respect LIMIT 1000
    assert!(notifications.len() <= 1000);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_with_special_characters() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel = fixture.unique_channel("channel_with_special_chars_!@#$%");
    let entity = fixture.unique_entity("entity_with_unicode_🚀");

    let notification = fixture
        .create_notification(
            &channel,
            "execution",
            &entity,
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    assert_eq!(notification.channel, channel);
    assert_eq!(notification.entity, entity);

    // Verify retrieval
    let found = NotificationRepository::find_by_id(&pool, notification.id)
        .await
        .expect("Failed to find notification")
        .expect("Notification not found");

    assert_eq!(found.channel, channel);
    assert_eq!(found.entity, entity);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_with_long_strings() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    // PostgreSQL pg_notify has a 63-character limit on channel names
    // Use reasonable-length channel name
    let channel = format!("channel_{}", fixture.id_suffix);

    // But entity and activity can be very long (TEXT fields)
    let long_entity = format!(
        "entity_{}_with_very_long_id_{}",
        fixture.id_suffix,
        "y".repeat(200)
    );
    let long_activity = format!("activity_with_long_name_{}", "z".repeat(200));

    let notification = fixture
        .create_notification(
            &channel,
            "execution",
            &long_entity,
            &long_activity,
            NotificationState::Created,
            None,
        )
        .await;

    assert_eq!(notification.channel, channel);
    assert_eq!(notification.entity, long_entity);
    assert_eq!(notification.activity, long_activity);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_state_with_multiple_states() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel = fixture.unique_channel("multi_state_channel");

    // Create notifications with all states
    let _n1 = fixture
        .create_notification(
            &channel,
            "execution",
            &fixture.unique_entity("e1"),
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    let _n2 = fixture
        .create_notification(
            &channel,
            "execution",
            &fixture.unique_entity("e2"),
            "created",
            NotificationState::Queued,
            None,
        )
        .await;

    let _n3 = fixture
        .create_notification(
            &channel,
            "execution",
            &fixture.unique_entity("e3"),
            "created",
            NotificationState::Processing,
            None,
        )
        .await;

    let _n4 = fixture
        .create_notification(
            &channel,
            "execution",
            &fixture.unique_entity("e4"),
            "created",
            NotificationState::Error,
            None,
        )
        .await;

    // Query each state
    let created = NotificationRepository::find_by_state(&pool, NotificationState::Created)
        .await
        .expect("Failed to find created");
    assert!(created.iter().any(|n| n.channel == channel));

    let queued = NotificationRepository::find_by_state(&pool, NotificationState::Queued)
        .await
        .expect("Failed to find queued");
    assert!(queued.iter().any(|n| n.channel == channel));

    let processing = NotificationRepository::find_by_state(&pool, NotificationState::Processing)
        .await
        .expect("Failed to find processing");
    assert!(processing.iter().any(|n| n.channel == channel));

    let errors = NotificationRepository::find_by_state(&pool, NotificationState::Error)
        .await
        .expect("Failed to find errors");
    assert!(errors.iter().any(|n| n.channel == channel));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_content_array() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let array_content = json!([
        {"step": 1, "status": "completed"},
        {"step": 2, "status": "running"},
        {"step": 3, "status": "pending"}
    ]);

    let notification = fixture.create_with_content(array_content.clone()).await;

    assert_eq!(notification.content.unwrap(), array_content);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_content_string_value() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let string_content = json!("Simple string message");

    let notification = fixture.create_with_content(string_content.clone()).await;

    assert_eq!(notification.content.unwrap(), string_content);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_content_number_value() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let number_content = json!(42);

    let notification = fixture.create_with_content(number_content.clone()).await;

    assert_eq!(notification.content.unwrap(), number_content);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_parallel_creation() {
    let pool = create_test_pool().await.expect("Failed to create pool");

    // Create multiple notifications concurrently
    let mut handles = vec![];

    for i in 0..10 {
        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            let fixture = NotificationFixture::new(pool_clone);
            let channel = fixture.unique_channel(&format!("parallel_channel_{}", i));
            let entity = fixture.unique_entity(&format!("parallel_entity_{}", i));

            fixture
                .create_notification(
                    &channel,
                    "execution",
                    &entity,
                    "created",
                    NotificationState::Created,
                    None,
                )
                .await
        });
        handles.push(handle);
    }

    let results = futures::future::join_all(handles).await;

    // All should succeed
    for result in results {
        assert!(result.is_ok());
        let notification = result.unwrap();
        assert!(notification.id > 0);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_channel_case_sensitive() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let channel_lower = fixture.unique_channel("testchannel");
    let channel_upper = format!("{}_UPPER", channel_lower);

    let n1 = fixture
        .create_notification(
            &channel_lower,
            "execution",
            &fixture.unique_entity("e1"),
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    let n2 = fixture
        .create_notification(
            &channel_upper,
            "execution",
            &fixture.unique_entity("e2"),
            "created",
            NotificationState::Created,
            None,
        )
        .await;

    // Channels should be treated as different
    assert_ne!(n1.channel, n2.channel);

    // Query by each channel
    let lower_results = NotificationRepository::find_by_channel(&pool, &channel_lower)
        .await
        .expect("Failed to find by channel");
    assert!(lower_results.iter().any(|n| n.id == n1.id));
    assert!(!lower_results.iter().any(|n| n.id == n2.id));

    let upper_results = NotificationRepository::find_by_channel(&pool, &channel_upper)
        .await
        .expect("Failed to find by channel");
    assert!(!upper_results.iter().any(|n| n.id == n1.id));
    assert!(upper_results.iter().any(|n| n.id == n2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_entity_type_variations() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    // Test various entity type names
    let entity_types = [
        "execution",
        "inquiry",
        "enforcement",
        "sensor",
        "action",
        "rule",
        "trigger",
        "custom_type",
        "webhook",
        "timer",
    ];

    for entity_type in entity_types {
        let channel = fixture.unique_channel("test_channel");
        let entity = fixture.unique_entity(&format!("entity_{}", entity_type));

        let notification = fixture
            .create_notification(
                &channel,
                entity_type,
                &entity,
                "created",
                NotificationState::Created,
                None,
            )
            .await;

        assert_eq!(notification.entity_type, entity_type);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_update_same_state() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let notification = fixture.create_default().await;
    let original_state = notification.state;
    let original_updated = notification.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "notification", notification.id, original_updated).await;

    // Update to the same state
    let update_input = UpdateNotificationInput {
        state: Some(original_state),
        content: None,
    };

    let updated = NotificationRepository::update(&pool, notification.id, update_input)
        .await
        .expect("Failed to update notification");

    assert_eq!(updated.state, original_state);
    // Updated timestamp should still change
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_multiple_updates() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let notification = fixture.create_default().await;

    // Perform multiple updates
    for i in 0..5 {
        let content = json!({"update_count": i});
        let update_input = UpdateNotificationInput {
            state: None,
            content: Some(content.clone()),
        };

        let updated = NotificationRepository::update(&pool, notification.id, update_input)
            .await
            .expect("Failed to update notification");

        assert_eq!(updated.content.unwrap(), content);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_get_by_id_alias() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = NotificationFixture::new(pool.clone());

    let created = fixture.create_default().await;

    // Test using get_by_id (which should call find_by_id internally and unwrap)
    let found = NotificationRepository::get_by_id(&pool, created.id)
        .await
        .expect("Failed to get notification");

    assert_eq!(found.id, created.id);
    assert_eq!(found.channel, created.channel);
}
