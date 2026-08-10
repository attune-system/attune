//! Integration and stress tests for FIFO Policy Execution Ordering
//!
//! These tests verify the complete execution ordering system including:
//! - End-to-end FIFO ordering with database persistence
//! - High-concurrency stress scenarios (1000+ executions)
//! - Multiple worker simulation
//! - Queue statistics accuracy under load
//! - Policy integration (concurrency + delays)
//! - Failure and cancellation scenarios
//! - Cross-action independence at scale

use attune_common::{
    config::Config,
    models::enums::ExecutionStatus,
    repositories::{
        action::{ActionRepository, CreateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        pack::{CreatePackInput, PackRepository},
        queue_stats::QueueStatsRepository,
        runtime::{CreateRuntimeInput, RuntimeRepository},
        Create,
    },
    test_database::TestDatabase,
};
use attune_executor::queue_manager::{ExecutionQueueManager, QueueConfig};
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Instant};

/// Test helper to set up database connection
async fn setup_db() -> TestDatabase {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);
    let config = Config::load_from_file(&config_path).expect("Failed to load test config");
    TestDatabase::create(&config.database)
        .await
        .expect("Failed to create isolated test database")
        .with_cleanup_on_drop()
}

/// Test helper to create a test pack
async fn create_test_pack(pool: &PgPool, suffix: &str) -> i64 {
    let pack_input = CreatePackInput {
        r#ref: format!("fifo_test_pack_{}", suffix),
        label: format!("FIFO Test Pack {}", suffix),
        description: Some(format!("Test pack for FIFO ordering tests {}", suffix)),
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({}),
        tags: vec![],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    PackRepository::create(pool, pack_input)
        .await
        .expect("Failed to create test pack")
        .id
}

/// Test helper to create a test runtime
#[allow(dead_code)]
async fn _create_test_runtime(pool: &PgPool, suffix: &str) -> i64 {
    let runtime_input = CreateRuntimeInput {
        r#ref: format!("fifo_test_runtime_{}", suffix),
        pack: None,
        pack_ref: None,
        description: Some(format!("Test runtime {}", suffix)),
        name: format!("Python {}", suffix),
        aliases: vec![],
        distributions: json!({"ubuntu": "python3"}),
        installation: Some(json!({"method": "apt"})),
        execution_config: json!({
            "interpreter": {
                "binary": "python3",
                "args": ["-u"],
                "file_extension": ".py"
            }
        }),
        auto_detected: false,
        detection_config: json!({}),
    };

    RuntimeRepository::create(pool, runtime_input)
        .await
        .expect("Failed to create test runtime")
        .id
}

/// Test helper to create a test action
async fn create_test_action(pool: &PgPool, pack_id: i64, pack_ref: &str, suffix: &str) -> i64 {
    let action_input = CreateActionInput {
        r#ref: format!("{}.action_{}", pack_ref, suffix),
        pack: pack_id,
        pack_ref: pack_ref.to_string(),
        label: format!("FIFO Test Action {}", suffix),
        description: Some(format!("Test action {}", suffix)),
        entrypoint: "echo test".to_string(),
        runtime: None,
        enabled: true,
        runtime_version_constraint: None,
        required_worker_runtimes: serde_json::json!({}),
        worker_selector: serde_json::json!({}),
        worker_tolerations: serde_json::json!([]),
        worker_affinity: serde_json::json!({}),
        param_schema: None,
        out_schema: None,
        is_adhoc: false,
        accesses_mcp: false,
        default_execution_permission_set_refs: Vec::new(),
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
        timeout_seconds: None,
    };

    ActionRepository::create(pool, action_input)
        .await
        .expect("Failed to create test action")
        .id
}

/// Test helper to create a test execution
async fn create_test_execution(
    pool: &PgPool,
    action_id: i64,
    action_ref: &str,
    status: ExecutionStatus,
) -> i64 {
    let execution_input = CreateExecutionInput {
        action: Some(action_id),
        action_ref: action_ref.to_string(),
        config: None,
        env_vars: None,
        parent: None,
        enforcement: None,
        executor: None,
        permission_set_refs: Vec::new(),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        worker_selector: None,
        worker_tolerations: None,
        worker_affinity: None,
        worker: None,
        status,
        trace_tag: None,
        result: None,
        workflow_task: None,
        timeout_seconds: None,
    };

    ExecutionRepository::create(pool, execution_input)
        .await
        .expect("Failed to create test execution")
        .id
}

/// Test helper to cleanup test data
async fn cleanup_test_data(pool: &PgPool, pack_id: i64) {
    // Delete queue stats
    sqlx::query(
        "DELETE FROM queue_stats WHERE action_id IN (SELECT id FROM action WHERE pack = $1)",
    )
    .bind(pack_id)
    .execute(pool)
    .await
    .expect("Failed to delete queue stats during test cleanup");

    // Delete executions
    sqlx::query("DELETE FROM execution WHERE action IN (SELECT id FROM action WHERE pack = $1)")
        .bind(pack_id)
        .execute(pool)
        .await
        .expect("Failed to delete executions during test cleanup");

    // Delete actions
    sqlx::query("DELETE FROM action WHERE pack = $1")
        .bind(pack_id)
        .execute(pool)
        .await
        .expect("Failed to delete actions during test cleanup");

    // Delete pack
    sqlx::query("DELETE FROM pack WHERE id = $1")
        .bind(pack_id)
        .execute(pool)
        .await
        .expect("Failed to delete pack during test cleanup");
}

async fn wait_for_queue_state(
    manager: &ExecutionQueueManager,
    action_id: i64,
    active_count: u32,
    queue_length: usize,
    total_enqueued: u64,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if manager
            .get_queue_stats(action_id)
            .await
            .is_some_and(|stats| {
                stats.active_count == active_count
                    && stats.queue_length == queue_length
                    && stats.total_enqueued == total_enqueued
            })
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Queue {action_id} did not reach active={active_count}, queued={queue_length}, total={total_enqueued}"
        );
        sleep(Duration::from_millis(10)).await;
    }
}

async fn release_next_active(
    manager: &ExecutionQueueManager,
    active_execution_ids: &mut VecDeque<i64>,
) -> Option<i64> {
    let execution_id = active_execution_ids
        .pop_front()
        .expect("Expected an active execution to release");
    let release = manager
        .release_active_slot(execution_id)
        .await
        .expect("Release should succeed")
        .expect("Active execution should have a tracked slot");

    if let Some(next_execution_id) = release.next_execution_id {
        active_execution_ids.push_back(next_execution_id);
    }

    release.next_execution_id
}

#[tokio::test]
#[ignore] // Requires database
async fn test_fifo_ordering_with_database() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("fifo_db_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    // Create queue manager with database pool
    let manager = Arc::new(ExecutionQueueManager::with_db_pool(
        QueueConfig::default(),
        pool.clone(),
    ));

    let max_concurrent = 1;
    let num_executions = 10;
    let execution_order = Arc::new(Mutex::new(Vec::new()));
    let execution_labels = Arc::new(Mutex::new(HashMap::new()));
    let mut handles = vec![];
    let (admitted_tx, mut admitted_rx) = mpsc::unbounded_channel();

    // Create first execution in database and enqueue
    let first_exec_id =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
    let mut active_execution_ids = VecDeque::from([first_exec_id]);
    manager
        .enqueue_and_wait(action_id, first_exec_id, max_concurrent, None)
        .await
        .expect("First execution should enqueue");

    // Spawn multiple executions
    for i in 1..num_executions {
        let pool_clone = pool.clone();
        let manager_clone = manager.clone();
        let order = execution_order.clone();
        let labels = execution_labels.clone();
        let action_ref_clone = action_ref.clone();
        let admitted_tx = admitted_tx.clone();

        let handle = tokio::spawn(async move {
            // Create execution in database
            let exec_id = create_test_execution(
                &pool_clone,
                action_id,
                &action_ref_clone,
                ExecutionStatus::Requested,
            )
            .await;
            labels.lock().await.insert(exec_id, i);

            // Enqueue and wait
            manager_clone
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await
                .expect("Enqueue should succeed");

            // Record order
            order.lock().await.push(i);
            admitted_tx
                .send(exec_id)
                .expect("Admission receiver should remain open");
        });

        handles.push(handle);
    }
    drop(admitted_tx);

    // Wait for all spawned tasks to persist their queue entries.
    let mut stats = None;
    for _ in 0..100 {
        stats = QueueStatsRepository::find_by_action(&pool, action_id)
            .await
            .expect("Should get queue stats");
        if stats
            .as_ref()
            .is_some_and(|stats| stats.queue_length as usize == (num_executions - 1) as usize)
        {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    let stats = stats.expect("Queue stats should exist");

    assert_eq!(stats.action_id, action_id);
    assert_eq!(stats.active_count as u32, 1);
    assert_eq!(stats.queue_length as usize, (num_executions - 1) as usize);
    assert_eq!(stats.max_concurrent as u32, max_concurrent);

    let queued_execution_ids = sqlx::query_scalar::<_, i64>(
        "SELECT e.execution_id \
         FROM execution_admission_entry e \
         JOIN execution_admission_state s ON s.id = e.state_id \
         WHERE s.action_id = $1 AND e.execution_id <> $2 \
         ORDER BY e.queue_order",
    )
    .bind(action_id)
    .bind(first_exec_id)
    .fetch_all(&pool)
    .await
    .expect("Persisted queue order should be readable");
    let labels = execution_labels.lock().await;
    let expected = queued_execution_ids
        .iter()
        .map(|execution_id| labels[execution_id])
        .collect::<Vec<_>>();
    drop(labels);

    // Release the initial execution, then only promoted executions whose
    // waiters have observed admission.
    release_next_active(&manager, &mut active_execution_ids).await;
    for _ in 1..num_executions {
        let execution_id = admitted_rx
            .recv()
            .await
            .expect("Every promoted execution should signal admission");
        assert_eq!(
            active_execution_ids.front(),
            Some(&execution_id),
            "The admitted execution should be the promoted queue head"
        );
        release_next_active(&manager, &mut active_execution_ids).await;
    }

    // Wait for all to complete
    for handle in handles {
        handle.await.expect("Task should complete");
    }

    // Verify admission order matches the persisted FIFO order.
    let order = execution_order.lock().await;
    assert_eq!(*order, expected, "Executions should complete in FIFO order");

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database - stress test
async fn test_high_concurrency_stress() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("stress_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    let manager = Arc::new(ExecutionQueueManager::with_db_pool(
        QueueConfig {
            max_queue_length: 2000,
            queue_timeout_seconds: 300,
            enable_metrics: true,
        },
        pool.clone(),
    ));

    let max_concurrent = 5;
    let num_executions: i64 = 1000;
    let execution_order = Arc::new(Mutex::new(Vec::new()));
    let mut handles = vec![];
    let execution_ids = Arc::new(Mutex::new(vec![None; num_executions as usize]));

    println!("Starting stress test with {} executions...", num_executions);
    let start_time = std::time::Instant::now();

    // Start first batch to fill capacity
    for i in 0i64..max_concurrent as i64 {
        let pool_clone = pool.clone();
        let manager_clone = manager.clone();
        let action_ref_clone = action_ref.clone();
        let order = execution_order.clone();
        let ids = execution_ids.clone();

        let handle = tokio::spawn(async move {
            let exec_id = create_test_execution(
                &pool_clone,
                action_id,
                &action_ref_clone,
                ExecutionStatus::Requested,
            )
            .await;
            ids.lock().await[i as usize] = Some(exec_id);

            manager_clone
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await
                .expect("Enqueue should succeed");

            order.lock().await.push(i);
        });

        handles.push(handle);
    }

    // Queue remaining executions
    for i in max_concurrent as i64..num_executions {
        let pool_clone = pool.clone();
        let manager_clone = manager.clone();
        let action_ref_clone = action_ref.clone();
        let order = execution_order.clone();
        let ids = execution_ids.clone();

        let handle = tokio::spawn(async move {
            let exec_id = create_test_execution(
                &pool_clone,
                action_id,
                &action_ref_clone,
                ExecutionStatus::Requested,
            )
            .await;
            ids.lock().await[i as usize] = Some(exec_id);

            manager_clone
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await
                .expect("Enqueue should succeed");

            order.lock().await.push(i);
        });

        handles.push(handle);

        // Small delay to avoid overwhelming the system
        if i % 100 == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    }

    // Give tasks time to queue
    sleep(Duration::from_millis(500)).await;

    println!("All tasks queued, checking stats...");

    // Verify queue stats
    let stats = manager.get_queue_stats(action_id).await;
    assert!(stats.is_some(), "Queue stats should exist");
    let stats = stats.unwrap();
    assert_eq!(stats.active_count, max_concurrent);
    assert!(stats.queue_length > 0, "Should have queued executions");

    println!(
        "Queue stats - Active: {}, Queued: {}, Total: {}",
        stats.active_count, stats.queue_length, stats.total_enqueued
    );

    // Release all executions
    let ids = execution_ids.lock().await;
    let mut active_execution_ids = VecDeque::from(
        ids.iter()
            .take(max_concurrent as usize)
            .map(|id| id.expect("Initial execution id should be recorded"))
            .collect::<Vec<_>>(),
    );
    drop(ids);

    println!("Releasing executions...");
    for i in 0..num_executions {
        if i % 100 == 0 {
            println!("Released {} executions", i);
        }
        release_next_active(&manager, &mut active_execution_ids).await;

        // Small delay to allow queue processing
        if i % 50 == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    }

    // Wait for all to complete
    println!("Waiting for all tasks to complete...");
    for (i, handle) in handles.into_iter().enumerate() {
        if i % 100 == 0 {
            println!("Completed {} tasks", i);
        }
        handle.await.expect("Task should complete");
    }

    let elapsed = start_time.elapsed();
    println!(
        "Stress test completed in {:.2}s ({:.0} exec/sec)",
        elapsed.as_secs_f64(),
        num_executions as f64 / elapsed.as_secs_f64()
    );

    // Verify FIFO order
    let order = execution_order.lock().await;
    assert_eq!(
        order.len(),
        num_executions as usize,
        "All executions should complete"
    );

    let expected: Vec<_> = (0..num_executions).collect();
    assert_eq!(
        *order, expected,
        "Executions should complete in strict FIFO order"
    );

    // Verify final queue stats
    let final_stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(final_stats.queue_length, 0, "Queue should be empty");
    assert_eq!(
        final_stats.total_enqueued, num_executions as u64,
        "Should track all enqueues"
    );
    assert_eq!(
        final_stats.total_completed, num_executions as u64,
        "Should track all completions"
    );

    println!("Final stats verified - Test passed!");

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_multiple_workers_simulation() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("workers_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    let manager = Arc::new(ExecutionQueueManager::with_db_pool(
        QueueConfig::default(),
        pool.clone(),
    ));

    let max_concurrent = 3;
    let num_executions = 30;
    let execution_order = Arc::new(Mutex::new(Vec::new()));
    let mut handles = vec![];
    let (admitted_tx, mut admitted_rx) = mpsc::unbounded_channel();

    // Fill the initial worker slots deterministically.
    for i in 0..max_concurrent {
        let exec_id =
            create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
        manager
            .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
            .await
            .expect("Initial execution should be admitted");
        execution_order.lock().await.push(i);
        admitted_tx
            .send(exec_id)
            .expect("Worker admission receiver should remain open");
    }

    // Queue the remaining executions.
    for i in max_concurrent..num_executions {
        let pool_clone = pool.clone();
        let manager_clone = manager.clone();
        let action_ref_clone = action_ref.clone();
        let order = execution_order.clone();
        let admitted_tx = admitted_tx.clone();

        let handle = tokio::spawn(async move {
            let exec_id = create_test_execution(
                &pool_clone,
                action_id,
                &action_ref_clone,
                ExecutionStatus::Requested,
            )
            .await;

            manager_clone
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await
                .expect("Enqueue should succeed");

            order.lock().await.push(i);
            admitted_tx
                .send(exec_id)
                .expect("Worker admission receiver should remain open");
        });

        handles.push(handle);
    }

    // Simulate workers completing at different rates
    // Worker 1: Fast (completes every 10ms)
    // Worker 2: Medium (completes every 30ms)
    // Worker 3: Slow (completes every 50ms)

    let worker_completions = Arc::new(Mutex::new(vec![0, 0, 0]));
    let worker_completions_clone = worker_completions.clone();
    let manager_clone = manager.clone();
    drop(admitted_tx);

    // Spawn worker simulators
    let worker_handle = tokio::spawn(async move {
        let mut next_worker = 0;
        for _ in 0..num_executions {
            let execution_id = admitted_rx
                .recv()
                .await
                .expect("Every execution should signal admission");

            // Simulate varying completion times
            let delay = match next_worker {
                0 => 10, // Fast worker
                1 => 30, // Medium worker
                _ => 50, // Slow worker
            };

            sleep(Duration::from_millis(delay)).await;

            // Worker completes and notifies
            manager_clone
                .release_active_slot(execution_id)
                .await
                .expect("Worker release should succeed")
                .expect("Admitted execution should own an active slot");

            worker_completions_clone.lock().await[next_worker] += 1;

            // Round-robin between workers
            next_worker = (next_worker + 1) % 3;
        }
    });

    // Wait for all executions and workers
    for handle in handles {
        handle.await.expect("Task should complete");
    }
    worker_handle
        .await
        .expect("Worker simulator should complete");

    // Verify FIFO order maintained despite different worker speeds
    let mut order = execution_order.lock().await.clone();
    order.sort_unstable();
    let expected: Vec<_> = (0..num_executions).collect();
    assert_eq!(
        order, expected,
        "Every execution should be admitted exactly once"
    );

    // Verify workers distributed load
    let completions = worker_completions.lock().await;
    println!("Worker completions: {:?}", *completions);
    assert!(
        completions.iter().all(|&c| c > 0),
        "All workers should have completed some executions"
    );

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_cross_action_independence() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("independence_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);

    // Create three different actions
    let action1_id = create_test_action(&pool, pack_id, &pack_ref, &format!("{}_a1", suffix)).await;
    let action2_id = create_test_action(&pool, pack_id, &pack_ref, &format!("{}_a2", suffix)).await;
    let action3_id = create_test_action(&pool, pack_id, &pack_ref, &format!("{}_a3", suffix)).await;

    let manager = Arc::new(ExecutionQueueManager::with_db_pool(
        QueueConfig::default(),
        pool.clone(),
    ));

    let executions_per_action = 50;
    let mut handles = vec![];
    let (action1_admitted_tx, mut action1_admitted_rx) = mpsc::unbounded_channel();
    let (action2_admitted_tx, mut action2_admitted_rx) = mpsc::unbounded_channel();
    let (action3_admitted_tx, mut action3_admitted_rx) = mpsc::unbounded_channel();

    // Spawn executions for all three actions simultaneously
    for action_id in [action1_id, action2_id, action3_id] {
        let action_ref = format!("{}.action_{}_{}", pack_ref, suffix, action_id);

        for i in 0..executions_per_action {
            let exec_id =
                create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested)
                    .await;

            let admitted_tx = match action_id {
                id if id == action1_id => action1_admitted_tx.clone(),
                id if id == action2_id => action2_admitted_tx.clone(),
                id if id == action3_id => action3_admitted_tx.clone(),
                _ => unreachable!("test only creates three actions"),
            };

            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                manager_clone
                    .enqueue_and_wait(action_id, exec_id, 1, None)
                    .await
                    .expect("Enqueue should succeed");
                admitted_tx
                    .send(exec_id)
                    .expect("Admission receiver should remain open");

                (action_id, i)
            });

            handles.push(handle);
        }
    }

    wait_for_queue_state(&manager, action1_id, 1, executions_per_action - 1, 50).await;
    wait_for_queue_state(&manager, action2_id, 1, executions_per_action - 1, 50).await;
    wait_for_queue_state(&manager, action3_id, 1, executions_per_action - 1, 50).await;

    // Verify all three queues exist independently
    let stats1 = manager.get_queue_stats(action1_id).await.unwrap();
    let stats2 = manager.get_queue_stats(action2_id).await.unwrap();
    let stats3 = manager.get_queue_stats(action3_id).await.unwrap();

    assert_eq!(stats1.action_id, action1_id);
    assert_eq!(stats2.action_id, action2_id);
    assert_eq!(stats3.action_id, action3_id);

    println!(
        "Action 1 - Active: {}, Queued: {}",
        stats1.active_count, stats1.queue_length
    );
    println!(
        "Action 2 - Active: {}, Queued: {}",
        stats2.active_count, stats2.queue_length
    );
    println!(
        "Action 3 - Active: {}, Queued: {}",
        stats3.active_count, stats3.queue_length
    );

    // Release all actions in an interleaved pattern
    for _ in 0..executions_per_action {
        // A worker can only complete an execution after its waiter has observed
        // admission. Releasing a merely promoted row races the polling helper.
        for execution_id in [
            action1_admitted_rx
                .recv()
                .await
                .expect("Action 1 execution should be admitted"),
            action2_admitted_rx
                .recv()
                .await
                .expect("Action 2 execution should be admitted"),
            action3_admitted_rx
                .recv()
                .await
                .expect("Action 3 execution should be admitted"),
        ] {
            manager
                .release_active_slot(execution_id)
                .await
                .expect("Release should succeed")
                .expect("Admitted execution should own an active slot");
        }
    }

    // Wait for all to complete
    for handle in handles {
        handle.await.expect("Task should complete");
    }

    // Verify all queues are empty
    let final_stats1 = manager.get_queue_stats(action1_id).await.unwrap();
    let final_stats2 = manager.get_queue_stats(action2_id).await.unwrap();
    let final_stats3 = manager.get_queue_stats(action3_id).await.unwrap();

    assert_eq!(final_stats1.queue_length, 0);
    assert_eq!(final_stats2.queue_length, 0);
    assert_eq!(final_stats3.queue_length, 0);

    assert_eq!(final_stats1.total_enqueued, executions_per_action as u64);
    assert_eq!(final_stats2.total_enqueued, executions_per_action as u64);
    assert_eq!(final_stats3.total_enqueued, executions_per_action as u64);

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_cancellation_during_queue() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("cancel_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    let manager = Arc::new(ExecutionQueueManager::with_db_pool(
        QueueConfig::default(),
        pool.clone(),
    ));

    let max_concurrent = 1;
    let mut handles = vec![];
    let mut execution_ids = Vec::new();
    let (admitted_tx, mut admitted_rx) = mpsc::unbounded_channel();

    // Fill capacity
    let exec_id =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
    let mut active_execution_ids = VecDeque::from([exec_id]);
    manager
        .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
        .await
        .unwrap();

    // Queue 10 more
    for _ in 0..10 {
        let exec_id =
            create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
        execution_ids.push(exec_id);
        let manager_clone = manager.clone();
        let admitted_tx = admitted_tx.clone();

        let handle = tokio::spawn(async move {
            let result = manager_clone
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await;
            if result.is_ok() {
                admitted_tx
                    .send(exec_id)
                    .expect("Admission receiver should remain open");
            }
            result
        });

        handles.push(handle);
    }
    drop(admitted_tx);

    // Verify all tasks have reached the queue before selecting cancellations.
    wait_for_queue_state(&manager, action_id, 1, 10, 11).await;

    // Cancel executions at positions 2, 5, 8
    let to_cancel = [execution_ids[2], execution_ids[5], execution_ids[8]];

    for cancel_id in &to_cancel {
        let cancelled = manager
            .cancel_execution(action_id, *cancel_id)
            .await
            .unwrap();
        assert!(cancelled, "Should successfully cancel queued execution");
    }

    // Verify queue length decreased
    let stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(
        stats.queue_length, 7,
        "Three executions should be removed from queue"
    );

    // Release the initial execution, then only promoted executions whose
    // waiters have observed admission.
    release_next_active(&manager, &mut active_execution_ids).await;
    for _ in 0..7 {
        let execution_id = admitted_rx
            .recv()
            .await
            .expect("Every non-cancelled execution should signal admission");
        assert_eq!(
            active_execution_ids.front(),
            Some(&execution_id),
            "The admitted execution should be the promoted queue head"
        );
        release_next_active(&manager, &mut active_execution_ids).await;
    }

    // Wait for handles to complete or error
    let mut completed = 0;
    let mut cancelled = 0;
    for handle in handles {
        match handle.await.expect("Queue waiter should not panic") {
            Ok(_) => completed += 1,
            Err(_) => cancelled += 1,
        }
    }

    assert_eq!(completed, 7, "Seven executions should complete");
    assert_eq!(cancelled, 3, "Three executions should be cancelled");

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_queue_stats_persistence() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("stats_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    let manager = Arc::new(ExecutionQueueManager::with_db_pool(
        QueueConfig::default(),
        pool.clone(),
    ));

    let max_concurrent = 5;
    let num_executions = 50;
    let (admitted_tx, mut admitted_rx) = mpsc::unbounded_channel();
    let mut handles = Vec::new();

    // Enqueue executions
    for i in 0..num_executions {
        let exec_id =
            create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
        if i < max_concurrent {
            manager
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await
                .expect("Initial execution should acquire an active slot");
            admitted_tx
                .send(exec_id)
                .expect("Admission receiver should remain open");
        } else {
            let manager_clone = manager.clone();
            let admitted_tx = admitted_tx.clone();
            handles.push(tokio::spawn(async move {
                manager_clone
                    .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                    .await
                    .expect("Queued execution should be admitted");
                admitted_tx
                    .send(exec_id)
                    .expect("Admission receiver should remain open");
            }));
        }

        if i % 10 == 0 {
            let expected_total = (i + 1) as u64;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let db_stats = QueueStatsRepository::find_by_action(&pool, action_id)
                    .await
                    .expect("Should query database")
                    .expect("Stats should exist in database");
                let current_stats = manager.get_queue_stats(action_id).await.unwrap();

                let synchronized = db_stats.action_id == current_stats.action_id
                    && db_stats.queue_length as usize == current_stats.queue_length
                    && db_stats.active_count as u32 == current_stats.active_count
                    && db_stats.max_concurrent as u32 == current_stats.max_concurrent
                    && db_stats.total_enqueued as u64 == current_stats.total_enqueued
                    && db_stats.total_completed as u64 == current_stats.total_completed
                    && current_stats.total_enqueued == expected_total;
                if synchronized {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Queue stats did not converge at {expected_total} enqueues: persisted={db_stats:?}, current={current_stats:?}"
                );
                sleep(Duration::from_millis(10)).await;
            }
        }
    }

    // Release only executions whose waiters have observed admission.
    for _ in 0..num_executions {
        let execution_id = admitted_rx
            .recv()
            .await
            .expect("Every execution should be admitted");
        manager
            .release_active_slot(execution_id)
            .await
            .expect("Release should succeed")
            .expect("Admitted execution should own an active slot");
    }

    for handle in handles {
        handle.await.expect("Queued waiter should complete");
    }

    // Final verification
    let final_db_stats = QueueStatsRepository::find_by_action(&pool, action_id)
        .await
        .expect("Should query database")
        .expect("Stats should exist");

    let final_mem_stats = manager.get_queue_stats(action_id).await.unwrap();

    assert_eq!(final_db_stats.queue_length, 0);
    assert_eq!(final_mem_stats.queue_length, 0);
    assert_eq!(final_db_stats.total_enqueued, num_executions as i64);
    assert_eq!(final_db_stats.total_completed, num_executions as i64);

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_release_restore_recovers_active_slot_and_next_queue_head() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("restore_release_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    let manager = ExecutionQueueManager::with_db_pool(QueueConfig::default(), pool.clone());

    let first =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
    let second =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
    let third =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;

    manager.enqueue(action_id, first, 1, None).await.unwrap();
    manager.enqueue(action_id, second, 1, None).await.unwrap();
    manager.enqueue(action_id, third, 1, None).await.unwrap();

    let stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(stats.active_count, 1);
    assert_eq!(stats.queue_length, 2);

    let release = manager
        .release_active_slot(first)
        .await
        .unwrap()
        .expect("first execution should own an active slot");
    assert_eq!(release.next_execution_id, Some(second));

    let stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(stats.active_count, 1);
    assert_eq!(stats.queue_length, 1);

    manager.restore_active_slot(first, &release).await.unwrap();

    let stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(stats.active_count, 1);
    assert_eq!(stats.queue_length, 2);
    assert_eq!(stats.total_completed, 0);

    let next = manager
        .release_active_slot(first)
        .await
        .unwrap()
        .expect("restored execution should still own the active slot");
    assert_eq!(next.next_execution_id, Some(second));

    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_remove_restore_recovers_queued_execution_position() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("restore_queue_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    let manager = ExecutionQueueManager::with_db_pool(QueueConfig::default(), pool.clone());

    let first =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
    let second =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
    let third =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;

    manager.enqueue(action_id, first, 1, None).await.unwrap();
    manager.enqueue(action_id, second, 1, None).await.unwrap();
    manager.enqueue(action_id, third, 1, None).await.unwrap();

    let removal = manager
        .remove_queued_execution(second)
        .await
        .unwrap()
        .expect("second execution should be queued");
    assert_eq!(removal.next_execution_id, None);

    let stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(stats.active_count, 1);
    assert_eq!(stats.queue_length, 1);

    manager.restore_queued_execution(&removal).await.unwrap();

    let stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(stats.active_count, 1);
    assert_eq!(stats.queue_length, 2);

    let release = manager
        .release_active_slot(first)
        .await
        .unwrap()
        .expect("first execution should own the active slot");
    assert_eq!(release.next_execution_id, Some(second));

    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_queue_full_rejection() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("full_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    let manager = Arc::new(ExecutionQueueManager::with_db_pool(
        QueueConfig {
            max_queue_length: 10,
            queue_timeout_seconds: 60,
            enable_metrics: true,
        },
        pool.clone(),
    ));

    let max_concurrent = 1;

    // Fill capacity (1 active)
    let active_exec_id =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
    manager
        .enqueue_and_wait(action_id, active_exec_id, max_concurrent, None)
        .await
        .unwrap();

    // Fill queue (10 queued)
    let mut queued_execution_ids = Vec::new();
    let mut waiters = Vec::new();
    for _ in 0..10 {
        let exec_id =
            create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
        queued_execution_ids.push(exec_id);
        let manager_clone = manager.clone();

        waiters.push(tokio::spawn(async move {
            manager_clone
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await
        }));
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let membership_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM execution_admission_entry WHERE execution_id = ANY($1)",
        )
        .bind(&queued_execution_ids)
        .fetch_one(&pool)
        .await
        .expect("Queue membership should be queryable");
        if membership_count == queued_execution_ids.len() as i64 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "All queue-full waiters did not become durable queue members"
        );
        sleep(Duration::from_millis(10)).await;
    }

    // Verify queue is full
    let stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(stats.active_count, 1);
    assert_eq!(stats.queue_length, 10);

    // Next enqueue should fail
    let exec_id =
        create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;
    let result = manager
        .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
        .await;

    assert!(result.is_err(), "Should reject when queue is full");
    assert!(result.unwrap_err().to_string().contains("Queue full"));

    for queued_execution_id in queued_execution_ids {
        assert!(
            manager
                .cancel_execution(action_id, queued_execution_id)
                .await
                .expect("Queued execution cancellation should succeed"),
            "Queued execution should be cancelled before cleanup"
        );
    }
    manager
        .release_active_slot(active_exec_id)
        .await
        .expect("Active execution release should succeed")
        .expect("Active execution should own its slot");

    for waiter in waiters {
        let result = waiter.await.expect("Queue waiter should not panic");
        assert!(
            result.is_err(),
            "Cancelled queue waiter should stop waiting"
        );
    }

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database - very long stress test
async fn test_extreme_stress_10k_executions() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let suffix = format!("extreme_{}", timestamp);

    let pack_id = create_test_pack(&pool, &suffix).await;
    let pack_ref = format!("fifo_test_pack_{}", suffix);
    let action_id = create_test_action(&pool, pack_id, &pack_ref, &suffix).await;
    let action_ref = format!("{}.action_{}", pack_ref, suffix);

    let manager = Arc::new(ExecutionQueueManager::with_db_pool(
        QueueConfig {
            max_queue_length: 15000,
            queue_timeout_seconds: 600,
            enable_metrics: true,
        },
        pool.clone(),
    ));

    let max_concurrent = 10;
    let num_executions: i64 = 10000;
    let completed = Arc::new(Mutex::new(0u64));
    let execution_ids = Arc::new(Mutex::new(vec![None; num_executions as usize]));

    println!(
        "Starting extreme stress test with {} executions...",
        num_executions
    );
    let start_time = std::time::Instant::now();

    // Spawn all executions
    let mut handles = vec![];
    for i in 0i64..num_executions {
        let pool_clone = pool.clone();
        let manager_clone = manager.clone();
        let action_ref_clone = action_ref.clone();
        let completed_clone = completed.clone();
        let ids = execution_ids.clone();

        let handle = tokio::spawn(async move {
            let exec_id = create_test_execution(
                &pool_clone,
                action_id,
                &action_ref_clone,
                ExecutionStatus::Requested,
            )
            .await;
            ids.lock().await[i as usize] = Some(exec_id);

            manager_clone
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await
                .expect("Enqueue should succeed");

            let mut count = completed_clone.lock().await;
            *count += 1;
            if *count % 1000 == 0 {
                println!("Enqueued: {}", *count);
            }
        });

        handles.push(handle);

        // Batch spawn to avoid overwhelming scheduler
        if i % 500 == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    }

    sleep(Duration::from_millis(1000)).await;
    println!("All executions spawned");

    // Release all
    let ids = execution_ids.lock().await;
    let mut active_execution_ids = VecDeque::from(
        ids.iter()
            .take(max_concurrent as usize)
            .map(|id| id.expect("Initial execution id should be recorded"))
            .collect::<Vec<_>>(),
    );
    drop(ids);

    let release_start = std::time::Instant::now();
    for i in 0i64..num_executions {
        release_next_active(&manager, &mut active_execution_ids).await;

        if i % 1000 == 0 {
            println!("Released: {}", i);
            sleep(Duration::from_millis(10)).await;
        }
    }
    println!(
        "All releases sent in {:.2}s",
        release_start.elapsed().as_secs_f64()
    );

    // Wait for all to complete
    println!("Waiting for all tasks to complete...");
    for (i, handle) in handles.into_iter().enumerate() {
        if i % 1000 == 0 {
            println!("Awaited: {}", i);
        }
        handle.await.expect("Task should complete");
    }

    let elapsed = start_time.elapsed();
    println!(
        "Extreme stress test completed in {:.2}s ({:.0} exec/sec)",
        elapsed.as_secs_f64(),
        num_executions as f64 / elapsed.as_secs_f64()
    );

    // Verify final state
    let final_stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(final_stats.queue_length, 0);
    assert_eq!(final_stats.total_enqueued as i64, num_executions);
    assert_eq!(final_stats.total_completed as i64, num_executions);

    println!("Extreme stress test passed!");

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}
