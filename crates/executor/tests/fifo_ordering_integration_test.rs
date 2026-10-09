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
        execution_admission::ExecutionAdmissionRepository,
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
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::{
    task::JoinHandle,
    time::{sleep, timeout, timeout_at, Instant},
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const LOAD_TIMEOUT: Duration = Duration::from_secs(900);

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

async fn wait_for_queue_state<T>(
    manager: &ExecutionQueueManager,
    action_id: i64,
    active_count: u32,
    queue_length: usize,
    total_enqueued: u64,
    handles: &mut [JoinHandle<T>],
) {
    let deadline = Instant::now()
        + if total_enqueued >= 1000 {
            LOAD_TIMEOUT
        } else {
            WAIT_TIMEOUT
        };
    loop {
        let last_stats = manager.get_queue_stats(action_id).await;
        if last_stats.as_ref().is_some_and(|stats| {
            stats.active_count == active_count
                && stats.queue_length == queue_length
                && stats.total_enqueued == total_enqueued
        }) {
            return;
        }
        if Instant::now() >= deadline {
            abort_and_await(handles).await;
            panic!(
                "Queue {action_id} did not reach active={active_count}, queued={queue_length}, total={total_enqueued}; last stats: {last_stats:?}"
            );
        }
        sleep(Duration::from_millis(10)).await;
    }
}

async fn abort_and_await<T>(handles: &mut [JoinHandle<T>]) {
    for handle in handles.iter() {
        if !handle.is_finished() {
            handle.abort();
        }
    }
    for handle in handles.iter_mut() {
        let _ = handle.await;
    }
}

async fn recv_admission_or_abort<T>(
    receiver: &mut mpsc::UnboundedReceiver<i64>,
    handles: &mut [JoinHandle<T>],
    manager: &ExecutionQueueManager,
    action_id: i64,
    context: &str,
    wait: Duration,
) -> i64 {
    match timeout(wait, receiver.recv()).await {
        Ok(Some(execution_id)) => execution_id,
        result => {
            let last_stats = manager.get_queue_stats(action_id).await;
            abort_and_await(handles).await;
            panic!(
                "Timed out or admission channel closed while {context}: result={result:?}, last stats={last_stats:?}"
            );
        }
    }
}

async fn join_handles_or_abort<T>(
    mut handles: Vec<JoinHandle<T>>,
    manager: &ExecutionQueueManager,
    action_id: i64,
    context: &str,
    wait: Duration,
) -> Vec<T> {
    let deadline = Instant::now() + wait;
    let handle_count = handles.len();
    let mut outputs = Vec::with_capacity(handle_count);

    for index in 0..handle_count {
        match timeout_at(deadline, &mut handles[index]).await {
            Ok(Ok(output)) => outputs.push(output),
            Ok(Err(error)) => {
                abort_and_await(&mut handles[index + 1..]).await;
                panic!("Task failed while {context}: {error}");
            }
            Err(_) => {
                let last_stats = manager.get_queue_stats(action_id).await;
                abort_and_await(&mut handles[index..]).await;
                panic!(
                    "Timed out while {context} after {}/{} joins; last stats={last_stats:?}",
                    outputs.len(),
                    handle_count
                );
            }
        }
    }

    outputs
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
    for _ in 1..num_executions {
        let pool_clone = pool.clone();
        let manager_clone = manager.clone();
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
            // Enqueue and wait
            manager_clone
                .enqueue_and_wait(action_id, exec_id, max_concurrent, None)
                .await
                .expect("Enqueue should succeed");

            admitted_tx
                .send(exec_id)
                .expect("Admission receiver should remain open");
        });

        handles.push(handle);
    }
    drop(admitted_tx);

    wait_for_queue_state(
        &manager,
        action_id,
        1,
        num_executions as usize - 1,
        10,
        &mut handles,
    )
    .await;
    let stats = QueueStatsRepository::find_by_action(&pool, action_id)
        .await
        .expect("Should get queue stats")
        .expect("Queue stats should exist");

    assert_eq!(stats.action_id, action_id);
    assert_eq!(stats.active_count as u32, 1);
    assert_eq!(stats.queue_length as usize, (num_executions - 1) as usize);
    assert_eq!(stats.max_concurrent as u32, max_concurrent);

    let expected = ExecutionAdmissionRepository::queued_execution_ids(&pool, action_id, None)
        .await
        .expect("Persisted queue order should be readable");

    // Release the initial execution, then only promoted executions whose
    // waiters have observed admission.
    release_next_active(&manager, &mut active_execution_ids).await;
    let mut admitted = Vec::with_capacity(num_executions as usize - 1);
    for _ in 1..num_executions {
        let execution_id = recv_admission_or_abort(
            &mut admitted_rx,
            &mut handles,
            &manager,
            action_id,
            "draining the deterministic FIFO queue",
            WAIT_TIMEOUT,
        )
        .await;
        assert_eq!(
            active_execution_ids.front(),
            Some(&execution_id),
            "The admitted execution should be the promoted queue head"
        );
        admitted.push(execution_id);
        release_next_active(&manager, &mut active_execution_ids).await;
    }

    join_handles_or_abort(
        handles,
        &manager,
        action_id,
        "joining deterministic FIFO waiters",
        WAIT_TIMEOUT,
    )
    .await;

    // Verify admission order matches the persisted FIFO order.
    assert_eq!(
        admitted, expected,
        "Admitted executions should match persisted FIFO order"
    );

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
    let mut handles = vec![];
    let (admitted_tx, mut admitted_rx) = mpsc::unbounded_channel();

    println!("Starting stress test with {} executions...", num_executions);
    let start_time = std::time::Instant::now();

    for i in 0i64..num_executions {
        let pool_clone = pool.clone();
        let manager_clone = manager.clone();
        let action_ref_clone = action_ref.clone();
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
            admitted_tx
                .send(exec_id)
                .expect("Admission receiver should remain open");
        });

        handles.push(handle);

        // Small delay to avoid overwhelming the system
        if i % 100 == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    }
    drop(admitted_tx);

    wait_for_queue_state(
        &manager,
        action_id,
        max_concurrent,
        num_executions as usize - max_concurrent as usize,
        num_executions as u64,
        &mut handles,
    )
    .await;

    println!("All tasks queued, checking stats...");

    // Verify queue stats
    let stats = manager.get_queue_stats(action_id).await;
    assert!(stats.is_some(), "Queue stats should exist");
    let stats = stats.unwrap();
    assert_eq!(stats.active_count, max_concurrent);
    assert_eq!(
        stats.queue_length,
        num_executions as usize - max_concurrent as usize
    );

    println!(
        "Queue stats - Active: {}, Queued: {}, Total: {}",
        stats.active_count, stats.queue_length, stats.total_enqueued
    );

    println!("Releasing executions...");
    for i in 0..num_executions {
        if i % 100 == 0 {
            println!("Released {} executions", i);
        }
        let execution_id = recv_admission_or_abort(
            &mut admitted_rx,
            &mut handles,
            &manager,
            action_id,
            "draining the 1000-execution stress queue",
            LOAD_TIMEOUT,
        )
        .await;
        manager
            .release_active_slot(execution_id)
            .await
            .expect("Release should succeed")
            .expect("Admitted execution should own an active slot");
    }

    // Wait for all to complete
    println!("Waiting for all tasks to complete...");
    let completed = join_handles_or_abort(
        handles,
        &manager,
        action_id,
        "joining 1000 stress waiters",
        LOAD_TIMEOUT,
    )
    .await;

    let elapsed = start_time.elapsed();
    println!(
        "Stress test completed in {:.2}s ({:.0} exec/sec)",
        elapsed.as_secs_f64(),
        num_executions as f64 / elapsed.as_secs_f64()
    );

    assert_eq!(
        completed.len(),
        num_executions as usize,
        "All executions should complete"
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
    // Model three workers requesting admission, not 27 simultaneous producers
    // holding the fixture pool behind one admission-state lock.
    let admission_requests = Arc::new(Semaphore::new(max_concurrent as usize));
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
        let admission_requests = admission_requests.clone();

        let handle = tokio::spawn(async move {
            let _admission_request = admission_requests
                .acquire_owned()
                .await
                .expect("Worker admission budget should remain open");
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
    drop(admitted_tx);

    let mut next_worker = 0;
    for _ in 0..num_executions {
        let execution_id = recv_admission_or_abort(
            &mut admitted_rx,
            &mut handles,
            &manager,
            action_id,
            "running the multiple-worker simulation",
            WAIT_TIMEOUT,
        )
        .await;

        let delay = match next_worker {
            0 => 10,
            1 => 30,
            _ => 50,
        };
        sleep(Duration::from_millis(delay)).await;

        manager
            .release_active_slot(execution_id)
            .await
            .expect("Worker release should succeed")
            .expect("Admitted execution should own an active slot");
        worker_completions.lock().await[next_worker] += 1;
        next_worker = (next_worker + 1) % 3;
    }

    join_handles_or_abort(
        handles,
        &manager,
        action_id,
        "joining multiple-worker waiters",
        WAIT_TIMEOUT,
    )
    .await;

    // This simulation checks load distribution and exactly-once admission. The
    // deterministic tests above cover persisted FIFO order.
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
    drop(manager);
    pool.cleanup()
        .await
        .expect("Multiple-worker fixture cleanup should complete");
}

#[tokio::test]
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

    wait_for_queue_state(
        &manager,
        action1_id,
        1,
        executions_per_action - 1,
        50,
        &mut handles,
    )
    .await;
    wait_for_queue_state(
        &manager,
        action2_id,
        1,
        executions_per_action - 1,
        50,
        &mut handles,
    )
    .await;
    wait_for_queue_state(
        &manager,
        action3_id,
        1,
        executions_per_action - 1,
        50,
        &mut handles,
    )
    .await;

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
            recv_admission_or_abort(
                &mut action1_admitted_rx,
                &mut handles,
                &manager,
                action1_id,
                "draining action 1",
                WAIT_TIMEOUT,
            )
            .await,
            recv_admission_or_abort(
                &mut action2_admitted_rx,
                &mut handles,
                &manager,
                action2_id,
                "draining action 2",
                WAIT_TIMEOUT,
            )
            .await,
            recv_admission_or_abort(
                &mut action3_admitted_rx,
                &mut handles,
                &manager,
                action3_id,
                "draining action 3",
                WAIT_TIMEOUT,
            )
            .await,
        ] {
            manager
                .release_active_slot(execution_id)
                .await
                .expect("Release should succeed")
                .expect("Admitted execution should own an active slot");
        }
    }

    // Wait for all to complete
    join_handles_or_abort(
        handles,
        &manager,
        action1_id,
        "joining cross-action waiters",
        WAIT_TIMEOUT,
    )
    .await;

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
    wait_for_queue_state(&manager, action_id, 1, 10, 11, &mut handles).await;
    let persisted_before_cancellation =
        ExecutionAdmissionRepository::queued_execution_ids(&pool, action_id, None)
            .await
            .expect("Persisted queue order should be readable before cancellation");

    // Cancel executions at positions 2, 5, 8
    let to_cancel = [execution_ids[2], execution_ids[5], execution_ids[8]];

    for cancel_id in &to_cancel {
        let cancelled = manager
            .cancel_execution(action_id, *cancel_id)
            .await
            .unwrap();
        assert!(cancelled, "Should successfully cancel queued execution");
    }

    let expected_remaining = persisted_before_cancellation
        .into_iter()
        .filter(|execution_id| !to_cancel.contains(execution_id))
        .collect::<Vec<_>>();
    let persisted_remaining =
        ExecutionAdmissionRepository::queued_execution_ids(&pool, action_id, None)
            .await
            .expect("Persisted queue order should be readable after cancellation");
    assert_eq!(
        persisted_remaining, expected_remaining,
        "Cancellation should preserve the relative order of remaining executions"
    );

    // Verify queue length decreased
    let stats = manager.get_queue_stats(action_id).await.unwrap();
    assert_eq!(
        stats.queue_length, 7,
        "Three executions should be removed from queue"
    );

    // Release the initial execution, then only promoted executions whose
    // waiters have observed admission.
    release_next_active(&manager, &mut active_execution_ids).await;
    let mut admitted = Vec::with_capacity(expected_remaining.len());
    for _ in 0..7 {
        let execution_id = recv_admission_or_abort(
            &mut admitted_rx,
            &mut handles,
            &manager,
            action_id,
            "draining the queue after cancellation",
            WAIT_TIMEOUT,
        )
        .await;
        assert_eq!(
            active_execution_ids.front(),
            Some(&execution_id),
            "The admitted execution should be the promoted queue head"
        );
        admitted.push(execution_id);
        release_next_active(&manager, &mut active_execution_ids).await;
    }
    assert_eq!(
        admitted, expected_remaining,
        "Admissions after cancellation should follow the persisted relative order"
    );

    // Wait for handles to complete or error
    let mut completed = 0;
    let mut cancelled = 0;
    for result in join_handles_or_abort(
        handles,
        &manager,
        action_id,
        "joining cancellation waiters",
        WAIT_TIMEOUT,
    )
    .await
    {
        match result {
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
            let deadline = Instant::now() + WAIT_TIMEOUT;
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
                if Instant::now() >= deadline {
                    abort_and_await(&mut handles).await;
                    panic!(
                        "Queue stats did not converge at {expected_total} enqueues: persisted={db_stats:?}, current={current_stats:?}"
                    );
                }
                sleep(Duration::from_millis(10)).await;
            }
        }
    }

    // Release only executions whose waiters have observed admission.
    for _ in 0..num_executions {
        let execution_id = recv_admission_or_abort(
            &mut admitted_rx,
            &mut handles,
            &manager,
            action_id,
            "draining the queue stats test",
            WAIT_TIMEOUT,
        )
        .await;
        manager
            .release_active_slot(execution_id)
            .await
            .expect("Release should succeed")
            .expect("Admitted execution should own an active slot");
    }

    join_handles_or_abort(
        handles,
        &manager,
        action_id,
        "joining queue stats waiters",
        WAIT_TIMEOUT,
    )
    .await;

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

    wait_for_queue_state(&manager, action_id, 1, 10, 11, &mut waiters).await;
    let persisted_ids = ExecutionAdmissionRepository::queued_execution_ids(&pool, action_id, None)
        .await
        .expect("Queue membership should be queryable");
    assert_eq!(persisted_ids.len(), queued_execution_ids.len());
    assert!(
        queued_execution_ids
            .iter()
            .all(|execution_id| persisted_ids.contains(execution_id)),
        "Every queue-full waiter should be a durable queue member"
    );

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

    for result in join_handles_or_abort(
        waiters,
        &manager,
        action_id,
        "joining cancelled queue-full waiters",
        WAIT_TIMEOUT,
    )
    .await
    {
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
    let (admitted_tx, mut admitted_rx) = mpsc::unbounded_channel();

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
            admitted_tx
                .send(exec_id)
                .expect("Admission receiver should remain open");
        });

        handles.push(handle);

        // Batch spawn to avoid overwhelming scheduler
        if i % 500 == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    }
    drop(admitted_tx);

    wait_for_queue_state(
        &manager,
        action_id,
        max_concurrent,
        num_executions as usize - max_concurrent as usize,
        num_executions as u64,
        &mut handles,
    )
    .await;
    println!("All executions spawned");

    let release_start = std::time::Instant::now();
    for i in 0i64..num_executions {
        let execution_id = recv_admission_or_abort(
            &mut admitted_rx,
            &mut handles,
            &manager,
            action_id,
            "draining the 10000-execution load queue",
            LOAD_TIMEOUT,
        )
        .await;
        manager
            .release_active_slot(execution_id)
            .await
            .expect("Release should succeed")
            .expect("Admitted execution should own an active slot");

        if i % 1000 == 0 {
            println!("Released: {}", i);
        }
    }
    println!(
        "All releases sent in {:.2}s",
        release_start.elapsed().as_secs_f64()
    );

    // Wait for all to complete
    println!("Waiting for all tasks to complete...");
    let completed = join_handles_or_abort(
        handles,
        &manager,
        action_id,
        "joining 10000 load waiters",
        LOAD_TIMEOUT,
    )
    .await;
    assert_eq!(completed.len(), num_executions as usize);

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
