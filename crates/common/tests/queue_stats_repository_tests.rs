//! Integration tests for queue stats repository
//!
//! Tests queue statistics persistence and retrieval operations.

use attune_common::repositories::queue_stats::{QueueStatsRepository, UpsertQueueStatsInput};
use chrono::Utc;

mod helpers;
use helpers::{ActionFixture, PackFixture};

#[tokio::test]
async fn test_upsert_queue_stats() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Create test pack and action using fixtures
    let pack = PackFixture::new_unique("test").create(&pool).await.unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    // Upsert queue stats (insert)
    let input = UpsertQueueStatsInput {
        action_id: action.id,
        queue_length: 5,
        active_count: 2,
        max_concurrent: 3,
        oldest_enqueued_at: Some(Utc::now()),
        total_enqueued: 100,
        total_completed: 95,
    };

    let stats = QueueStatsRepository::upsert(&pool, input.clone())
        .await
        .unwrap();

    assert_eq!(stats.action_id, action.id);
    assert_eq!(stats.queue_length, 5);
    assert_eq!(stats.active_count, 2);
    assert_eq!(stats.max_concurrent, 3);
    assert_eq!(stats.total_enqueued, 100);
    assert_eq!(stats.total_completed, 95);
    assert!(stats.oldest_enqueued_at.is_some());

    // Upsert again (update)
    let update_input = UpsertQueueStatsInput {
        action_id: action.id,
        queue_length: 3,
        active_count: 3,
        max_concurrent: 3,
        oldest_enqueued_at: None,
        total_enqueued: 110,
        total_completed: 107,
    };

    let updated_stats = QueueStatsRepository::upsert(&pool, update_input)
        .await
        .unwrap();

    assert_eq!(updated_stats.action_id, action.id);
    assert_eq!(updated_stats.queue_length, 3);
    assert_eq!(updated_stats.active_count, 3);
    assert_eq!(updated_stats.total_enqueued, 110);
    assert_eq!(updated_stats.total_completed, 107);
    assert!(updated_stats.oldest_enqueued_at.is_none());
}

#[tokio::test]
async fn test_find_queue_stats_by_action() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Create test pack and action
    let pack = PackFixture::new_unique("test").create(&pool).await.unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    // No stats initially
    let result = QueueStatsRepository::find_by_action(&pool, action.id)
        .await
        .unwrap();
    assert!(result.is_none());

    // Create stats
    let input = UpsertQueueStatsInput {
        action_id: action.id,
        queue_length: 10,
        active_count: 5,
        max_concurrent: 5,
        oldest_enqueued_at: Some(Utc::now()),
        total_enqueued: 200,
        total_completed: 190,
    };

    QueueStatsRepository::upsert(&pool, input).await.unwrap();

    // Find stats
    let stats = QueueStatsRepository::find_by_action(&pool, action.id)
        .await
        .unwrap()
        .expect("Stats should exist");

    assert_eq!(stats.action_id, action.id);
    assert_eq!(stats.queue_length, 10);
    assert_eq!(stats.active_count, 5);
}

#[tokio::test]
async fn test_list_active_queue_stats() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Create test pack
    let pack = PackFixture::new_unique("test").create(&pool).await.unwrap();

    // Create multiple actions with different queue states
    for i in 0..3 {
        let action = ActionFixture::new_unique(pack.id, &pack.r#ref, &format!("action_{}", i))
            .create(&pool)
            .await
            .unwrap();

        let input = if i == 0 {
            // Active queue
            UpsertQueueStatsInput {
                action_id: action.id,
                queue_length: 5,
                active_count: 2,
                max_concurrent: 3,
                oldest_enqueued_at: Some(Utc::now()),
                total_enqueued: 50,
                total_completed: 45,
            }
        } else if i == 1 {
            // Active executions but no queue
            UpsertQueueStatsInput {
                action_id: action.id,
                queue_length: 0,
                active_count: 3,
                max_concurrent: 3,
                oldest_enqueued_at: None,
                total_enqueued: 30,
                total_completed: 27,
            }
        } else {
            // Idle (should not appear in active list)
            UpsertQueueStatsInput {
                action_id: action.id,
                queue_length: 0,
                active_count: 0,
                max_concurrent: 3,
                oldest_enqueued_at: None,
                total_enqueued: 20,
                total_completed: 20,
            }
        };

        QueueStatsRepository::upsert(&pool, input).await.unwrap();
    }

    // List active queues
    let active_stats = QueueStatsRepository::list_active(&pool).await.unwrap();

    // Should only return entries with queue_length > 0 or active_count > 0
    // At least 2 from our test data (may be more from other tests)
    let our_active = active_stats
        .iter()
        .filter(|s| s.queue_length > 0 || s.active_count > 0)
        .count();
    assert!(our_active >= 2);
}

#[tokio::test]
async fn test_delete_queue_stats() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Create test pack and action
    let pack = PackFixture::new_unique("test").create(&pool).await.unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    // Create stats
    let input = UpsertQueueStatsInput {
        action_id: action.id,
        queue_length: 5,
        active_count: 2,
        max_concurrent: 3,
        oldest_enqueued_at: Some(Utc::now()),
        total_enqueued: 100,
        total_completed: 95,
    };

    QueueStatsRepository::upsert(&pool, input).await.unwrap();

    // Verify exists
    let stats = QueueStatsRepository::find_by_action(&pool, action.id)
        .await
        .unwrap();
    assert!(stats.is_some());

    // Delete
    let deleted = QueueStatsRepository::delete(&pool, action.id)
        .await
        .unwrap();
    assert!(deleted);

    // Verify deleted
    let stats = QueueStatsRepository::find_by_action(&pool, action.id)
        .await
        .unwrap();
    assert!(stats.is_none());

    // Delete again (should return false)
    let deleted = QueueStatsRepository::delete(&pool, action.id)
        .await
        .unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_batch_upsert_queue_stats() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Create test pack
    let pack = PackFixture::new_unique("test").create(&pool).await.unwrap();

    // Create multiple actions
    let mut inputs = Vec::new();
    for i in 0..5 {
        let action = ActionFixture::new_unique(pack.id, &pack.r#ref, &format!("action_{}", i))
            .create(&pool)
            .await
            .unwrap();

        inputs.push(UpsertQueueStatsInput {
            action_id: action.id,
            queue_length: i,
            active_count: i,
            max_concurrent: 5,
            oldest_enqueued_at: if i > 0 { Some(Utc::now()) } else { None },
            total_enqueued: (i * 10) as i64,
            total_completed: (i * 9) as i64,
        });
    }

    // Batch upsert
    let results = QueueStatsRepository::batch_upsert(&pool, inputs)
        .await
        .unwrap();

    assert_eq!(results.len(), 5);

    // Verify each result
    for (i, stats) in results.iter().enumerate() {
        assert_eq!(stats.queue_length, i as i32);
        assert_eq!(stats.active_count, i as i32);
        assert_eq!(stats.total_enqueued, (i * 10) as i64);
        assert_eq!(stats.total_completed, (i * 9) as i64);
    }
}

#[tokio::test]
async fn test_clear_stale_queue_stats() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Create test pack
    let pack = PackFixture::new_unique("test").create(&pool).await.unwrap();

    // Create action with idle stats
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    // Create idle stats (queue_length = 0, active_count = 0)
    let input = UpsertQueueStatsInput {
        action_id: action.id,
        queue_length: 0,
        active_count: 0,
        max_concurrent: 3,
        oldest_enqueued_at: None,
        total_enqueued: 100,
        total_completed: 100,
    };

    QueueStatsRepository::upsert(&pool, input).await.unwrap();

    // Try to clear stale stats (with very large timeout - should not delete recent stats)
    let _cleared = QueueStatsRepository::clear_stale(&pool, 3600)
        .await
        .unwrap();
    // May or may not be 0 depending on other test data, but our stat should still exist

    // Verify our stat still exists (was just created)
    let stats = QueueStatsRepository::find_by_action(&pool, action.id)
        .await
        .unwrap();
    assert!(stats.is_some());
}

#[tokio::test]
async fn test_queue_stats_cascade_delete() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Create test pack and action
    let pack = PackFixture::new_unique("test").create(&pool).await.unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    // Create stats
    let input = UpsertQueueStatsInput {
        action_id: action.id,
        queue_length: 5,
        active_count: 2,
        max_concurrent: 3,
        oldest_enqueued_at: Some(Utc::now()),
        total_enqueued: 100,
        total_completed: 95,
    };

    QueueStatsRepository::upsert(&pool, input).await.unwrap();

    // Verify stats exist
    let stats = QueueStatsRepository::find_by_action(&pool, action.id)
        .await
        .unwrap();
    assert!(stats.is_some());

    // Delete the action (should cascade to queue_stats)
    use attune_common::repositories::action::ActionRepository;
    use attune_common::repositories::Delete;
    ActionRepository::delete(&pool, action.id).await.unwrap();

    // Verify stats are also deleted (cascade)
    let stats = QueueStatsRepository::find_by_action(&pool, action.id)
        .await
        .unwrap();
    assert!(stats.is_none());
}
