//! Integration tests for SSE execution stream endpoint
//!
//! These tests verify that:
//! 1. PostgreSQL LISTEN/NOTIFY correctly triggers notifications
//! 2. The SSE endpoint streams execution updates in real-time
//! 3. Filtering by execution_id works correctly
//! 4. Authentication is properly enforced
//! 5. Reconnection and error handling work as expected

use attune_common::{
    models::*,
    repositories::{
        action::{ActionRepository, CreateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        pack::{CreatePackInput, PackRepository},
        Create,
    },
};

use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures::StreamExt;
use serde_json::{json, Value};
use sqlx::{postgres::PgListener, PgPool};
use std::pin::Pin;
use std::time::Duration;
use tokio::time::timeout;

mod helpers;
use helpers::TestContext;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

type SseStream = Pin<
    Box<
        dyn futures::Stream<Item = std::result::Result<Event, EventStreamError<reqwest::Error>>>
            + Send,
    >,
>;

async fn authenticated_event_source(url: &str, token: &str) -> Result<SseStream> {
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?;
    Ok(Box::pin(response.bytes_stream().eventsource()))
}

/// Helper to set up test pack and action
async fn setup_test_pack_and_action(pool: &PgPool) -> Result<(Pack, Action)> {
    let pack_input = CreatePackInput {
        r#ref: "test_sse_pack".to_string(),
        label: "Test SSE Pack".to_string(),
        description: Some("Pack for SSE testing".to_string()),
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({"author": "test"}),
        tags: vec!["test".to_string()],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };
    let pack = PackRepository::create(pool, pack_input).await?;

    let action_input = CreateActionInput {
        r#ref: format!("{}.test_action", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Test Action".to_string(),
        description: Some("Test action for SSE tests".to_string()),
        entrypoint: "test.sh".to_string(),
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
    let action = ActionRepository::create(pool, action_input).await?;

    Ok((pack, action))
}

/// Helper to create a test execution
async fn create_test_execution(pool: &PgPool, action_id: i64) -> Result<Execution> {
    create_test_execution_with_trace_tag(pool, action_id, None).await
}

async fn create_test_execution_with_trace_tag(
    pool: &PgPool,
    action_id: i64,
    trace_tag: Option<String>,
) -> Result<Execution> {
    create_test_execution_with_notification_data(pool, action_id, trace_tag, None).await
}

async fn create_test_execution_with_notification_data(
    pool: &PgPool,
    action_id: i64,
    trace_tag: Option<String>,
    workflow_task: Option<WorkflowTaskMetadata>,
) -> Result<Execution> {
    let input = CreateExecutionInput {
        action: Some(action_id),
        action_ref: format!("action_{}", action_id),
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
        status: ExecutionStatus::Scheduled,
        trace_tag,
        result: None,
        workflow_task,
        timeout_seconds: None,
    };
    Ok(ExecutionRepository::create(pool, input).await?)
}

async fn receive_execution_notification(
    listener: &mut PgListener,
    channel: &str,
    trace_tag: &str,
) -> Result<Value> {
    timeout(Duration::from_secs(5), async {
        loop {
            let notification = listener.recv().await?;
            let payload: Value = serde_json::from_str(notification.payload())?;
            if notification.channel() == channel && payload["trace_tag"] == trace_tag {
                return Ok(payload);
            }
        }
    })
    .await?
}

/// This test requires a running API server on port 8080
/// Run with: cargo test test_sse_stream_receives_execution_updates -- --ignored --nocapture
/// After starting: cargo run -p attune-api -- -c config.test.yaml
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sse_stream_receives_execution_updates() -> Result<()> {
    // Set up test context with auth
    let ctx = TestContext::new().await?.with_auth().await?;
    let token = ctx.token().unwrap();

    // Create test pack, action, and execution
    let (_pack, action) = setup_test_pack_and_action(&ctx.pool).await?;
    let execution = create_test_execution(&ctx.pool, action.id).await?;

    println!(
        "Created execution: id={}, status={:?}",
        execution.id, execution.status
    );

    // Build SSE URL with authentication
    let sse_url = format!(
        "http://localhost:8080/api/v1/executions/stream?execution_id={}",
        execution.id
    );

    // Create SSE stream
    let mut stream = authenticated_event_source(&sse_url, token).await?;

    // Spawn a task to update the execution status after a short delay
    let pool_clone = ctx.pool.clone();
    let execution_id = execution.id;
    tokio::spawn(async move {
        // Wait a bit to ensure SSE connection is established
        tokio::time::sleep(Duration::from_millis(500)).await;

        println!("Updating execution {} to 'running' status", execution_id);

        // Update execution status - this should trigger PostgreSQL NOTIFY
        let _ =
            sqlx::query("UPDATE execution SET status = 'running', updated = NOW() WHERE id = $1")
                .bind(execution_id)
                .execute(&pool_clone)
                .await;

        println!("Update executed, waiting before setting to succeeded");
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Update to succeeded
        let _ =
            sqlx::query("UPDATE execution SET status = 'succeeded', updated = NOW() WHERE id = $1")
                .bind(execution_id)
                .execute(&pool_clone)
                .await;

        println!("Execution {} updated to 'succeeded'", execution_id);
    });

    // Wait for SSE events with timeout
    let mut received_running = false;
    let mut received_succeeded = false;
    let mut attempts = 0;
    let max_attempts = 20; // 10 seconds total

    while attempts < max_attempts && (!received_running || !received_succeeded) {
        match timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(event))) => {
                println!("Received SSE event: {:?}", event);

                if let Ok(data) = serde_json::from_str::<Value>(&event.data) {
                    println!(
                        "Parsed event data: {}",
                        serde_json::to_string_pretty(&data)?
                    );

                    if let Some(entity_type) = data.get("entity_type").and_then(|v| v.as_str()) {
                        if entity_type == "execution" {
                            if let Some(event_data) = data.get("data") {
                                if let Some(status) =
                                    event_data.get("status").and_then(|v| v.as_str())
                                {
                                    println!("Received execution update with status: {}", status);

                                    if status == "running" {
                                        received_running = true;
                                        println!("✓ Received 'running' status");
                                    } else if status == "succeeded" {
                                        received_succeeded = true;
                                        println!("✓ Received 'succeeded' status");
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(Some(Err(e))) => {
                eprintln!("SSE stream error: {}", e);
                break;
            }
            Ok(None) => {
                println!("SSE stream ended");
                break;
            }
            Err(_) => {
                // Timeout waiting for next event
                attempts += 1;
                println!(
                    "Timeout waiting for event (attempt {}/{})",
                    attempts, max_attempts
                );
            }
        }
    }

    // Verify we received both updates
    assert!(
        received_running,
        "Should have received execution update with status 'running'"
    );
    assert!(
        received_succeeded,
        "Should have received execution update with status 'succeeded'"
    );

    println!("✓ Test passed: SSE stream received all expected updates");

    Ok(())
}

/// Test that SSE stream correctly filters by execution_id
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sse_stream_filters_by_execution_id() -> Result<()> {
    // Set up test context with auth
    let ctx = TestContext::new().await?.with_auth().await?;
    let token = ctx.token().unwrap();

    // Create test pack, action, and TWO executions
    let (_pack, action) = setup_test_pack_and_action(&ctx.pool).await?;
    let execution1 = create_test_execution(&ctx.pool, action.id).await?;
    let execution2 = create_test_execution(&ctx.pool, action.id).await?;

    println!(
        "Created executions: id1={}, id2={}",
        execution1.id, execution2.id
    );

    // Subscribe to updates for execution1 only
    let sse_url = format!(
        "http://localhost:8080/api/v1/executions/stream?execution_id={}",
        execution1.id
    );

    let mut stream = authenticated_event_source(&sse_url, token).await?;

    // Update both executions
    let pool_clone = ctx.pool.clone();
    let exec1_id = execution1.id;
    let exec2_id = execution2.id;

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Update execution2 (should NOT appear in filtered stream)
        let _ = sqlx::query("UPDATE execution SET status = 'completed' WHERE id = $1")
            .bind(exec2_id)
            .execute(&pool_clone)
            .await;

        println!("Updated execution2 {} to 'completed'", exec2_id);

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Update execution1 (SHOULD appear in filtered stream)
        let _ = sqlx::query("UPDATE execution SET status = 'running' WHERE id = $1")
            .bind(exec1_id)
            .execute(&pool_clone)
            .await;

        println!("Updated execution1 {} to 'running'", exec1_id);
    });

    // Wait for events
    let mut received_exec1_update = false;
    let mut received_exec2_update = false;
    let mut attempts = 0;
    let max_attempts = 20;

    while attempts < max_attempts && !received_exec1_update {
        match timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(event))) => {
                if let Ok(data) = serde_json::from_str::<Value>(&event.data) {
                    if let Some(entity_id) = data.get("entity_id").and_then(|v| v.as_i64()) {
                        println!("Received update for execution: {}", entity_id);

                        if entity_id == execution1.id {
                            received_exec1_update = true;
                            println!("✓ Received update for execution1 (correct)");
                        } else if entity_id == execution2.id {
                            received_exec2_update = true;
                            println!("✗ Received update for execution2 (should be filtered out)");
                        }
                    }
                }
            }
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => {
                attempts += 1;
            }
        }
    }

    // Should receive execution1 update but NOT execution2
    assert!(
        received_exec1_update,
        "Should have received update for execution1"
    );
    assert!(
        !received_exec2_update,
        "Should NOT have received update for execution2 (filtered out)"
    );

    println!("✓ Test passed: SSE stream correctly filters by execution_id");

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sse_stream_requires_authentication() -> Result<()> {
    let sse_url = "http://localhost:8080/api/v1/executions/stream";
    let response = reqwest::Client::new().get(sse_url).send().await?;

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    println!("✓ Test passed: SSE stream requires authentication");

    Ok(())
}

/// Test streaming all executions (no filter)
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sse_stream_all_executions() -> Result<()> {
    // Set up test context with auth
    let ctx = TestContext::new().await?.with_auth().await?;
    let token = ctx.token().unwrap();

    // Create test pack, action, and multiple executions
    let (_pack, action) = setup_test_pack_and_action(&ctx.pool).await?;
    let execution1 = create_test_execution(&ctx.pool, action.id).await?;
    let execution2 = create_test_execution(&ctx.pool, action.id).await?;

    println!(
        "Created executions: id1={}, id2={}",
        execution1.id, execution2.id
    );

    // Subscribe to ALL execution updates (no execution_id filter)
    let sse_url = "http://localhost:8080/api/v1/executions/stream";

    let mut stream = authenticated_event_source(sse_url, token).await?;

    // Update both executions
    let pool_clone = ctx.pool.clone();
    let exec1_id = execution1.id;
    let exec2_id = execution2.id;

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Update execution1
        let _ = sqlx::query("UPDATE execution SET status = 'running' WHERE id = $1")
            .bind(exec1_id)
            .execute(&pool_clone)
            .await;

        println!("Updated execution1 {} to 'running'", exec1_id);

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Update execution2
        let _ = sqlx::query("UPDATE execution SET status = 'running' WHERE id = $1")
            .bind(exec2_id)
            .execute(&pool_clone)
            .await;

        println!("Updated execution2 {} to 'running'", exec2_id);
    });

    // Wait for events from BOTH executions
    let mut received_updates = std::collections::HashSet::new();
    let mut attempts = 0;
    let max_attempts = 20;

    while attempts < max_attempts && received_updates.len() < 2 {
        match timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(event))) => {
                if let Ok(data) = serde_json::from_str::<Value>(&event.data) {
                    if let Some(entity_id) = data.get("entity_id").and_then(|v| v.as_i64()) {
                        println!("Received update for execution: {}", entity_id);
                        received_updates.insert(entity_id);
                    }
                }
            }
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => {
                attempts += 1;
            }
        }
    }

    // Should have received updates for BOTH executions
    assert!(
        received_updates.contains(&execution1.id),
        "Should have received update for execution1"
    );
    assert!(
        received_updates.contains(&execution2.id),
        "Should have received update for execution2"
    );

    println!("✓ Test passed: SSE stream received updates for all executions (no filter)");

    Ok(())
}

/// Test that PostgreSQL NOTIFY triggers actually fire
#[tokio::test]
async fn test_postgresql_notify_trigger_fires() -> Result<()> {
    let ctx = TestContext::new().await?;

    // Create test pack and action before listening for execution notifications.
    let (_pack, action) = setup_test_pack_and_action(&ctx.pool).await?;
    let mut listener = sqlx::postgres::PgListener::connect_with(&ctx.pool).await?;
    listener
        .listen_all(["execution_created", "execution_status_changed"])
        .await?;

    let trace_tag = format!("sse-notification-{}", uuid::Uuid::new_v4().simple());
    let execution =
        create_test_execution_with_trace_tag(&ctx.pool, action.id, Some(trace_tag.clone())).await?;
    let created_payload =
        receive_execution_notification(&mut listener, "execution_created", &trace_tag).await?;
    assert_eq!(created_payload["entity_id"], execution.id);
    assert_eq!(created_payload["trace_tag"], trace_tag.as_str());
    assert_eq!(created_payload["auth_mode"], "full");

    sqlx::query("UPDATE execution SET status = 'running' WHERE id = $1")
        .bind(execution.id)
        .execute(&ctx.pool)
        .await?;
    let updated_payload =
        receive_execution_notification(&mut listener, "execution_status_changed", &trace_tag)
            .await?;
    assert_eq!(updated_payload["entity_id"], execution.id);
    assert_eq!(updated_payload["trace_tag"], trace_tag.as_str());
    assert_eq!(updated_payload["auth_mode"], "full");

    let compact_trace_tag = format!("sse-compact-{}", uuid::Uuid::new_v4().simple());
    let compact_execution = create_test_execution_with_notification_data(
        &ctx.pool,
        action.id,
        Some(compact_trace_tag.clone()),
        Some(WorkflowTaskMetadata {
            workflow_execution: 1,
            task_name: "x".repeat(7_000),
            triggered_by: None,
            task_index: None,
            task_batch: None,
            retry_count: 0,
            max_retries: 0,
            next_retry_at: None,
            timeout_seconds: None,
            timed_out: false,
            duration_ms: None,
            started_at: None,
            completed_at: None,
        }),
    )
    .await?;
    let compact_created =
        receive_execution_notification(&mut listener, "execution_created", &compact_trace_tag)
            .await?;
    assert_eq!(compact_created["entity_id"], compact_execution.id);
    assert_eq!(compact_created["trace_tag"], compact_trace_tag.as_str());
    assert_eq!(compact_created["auth_mode"], "deferred");

    sqlx::query("UPDATE execution SET status = 'running' WHERE id = $1")
        .bind(compact_execution.id)
        .execute(&ctx.pool)
        .await?;
    let compact_updated = receive_execution_notification(
        &mut listener,
        "execution_status_changed",
        &compact_trace_tag,
    )
    .await?;
    assert_eq!(compact_updated["entity_id"], compact_execution.id);
    assert_eq!(compact_updated["trace_tag"], compact_trace_tag.as_str());
    assert_eq!(compact_updated["auth_mode"], "deferred");

    Ok(())
}
