mod helpers;

use std::time::Duration;

use attune_common::{
    auth::jwt::{generate_execution_token, JwtConfig},
    models::enums::ExecutionStatus,
    repositories::{
        execution::{CreateExecutionInput, ExecutionRepository},
        execution_log_stream_lease::ExecutionLogStreamLeaseRepository,
        Create,
    },
};
use axum::http::StatusCode;
use helpers::{create_test_action, create_test_pack, TestContext};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

async fn stream_fixture(ctx: &TestContext) -> Result<(i64, String)> {
    let pack = create_test_pack(
        &ctx.pool,
        &format!("stream_{}", uuid::Uuid::new_v4().simple()),
    )
    .await?;
    let action_ref = format!("{}.tail", pack.r#ref);
    let action = create_test_action(&ctx.pool, pack.id, &pack.r#ref, &action_ref).await?;
    let identity = ctx.user.as_ref().expect("authenticated identity");
    let execution = ExecutionRepository::create(
        &ctx.pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: Some(identity.id),
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Running,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await?;
    let token = generate_execution_token(
        identity.id,
        execution.id,
        &action.r#ref,
        &JwtConfig {
            secret: "test-secret-for-testing-only-not-secure".to_string(),
            access_token_expiration: 300,
            refresh_token_expiration: 3600,
        },
        None,
    )?;
    Ok((execution.id, token))
}

async fn wait_for_no_leases(ctx: &TestContext) -> Result<()> {
    for _ in 0..50 {
        if ExecutionLogStreamLeaseRepository::active_count(&ctx.pool).await? == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err("execution log stream lease was not released".into())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn route_returns_429_and_dropping_sse_body_releases_lease() -> Result<()> {
    let ctx = TestContext::new_with_stream_limits(2, 1)
        .await?
        .with_auth()
        .await?;
    let (execution_id, token) = stream_fixture(&ctx).await?;
    let path = format!("/api/v1/executions/{execution_id}/logs/stdout/stream");

    let first = ctx.get(&path, Some(&token)).await?;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        ExecutionLogStreamLeaseRepository::active_count(&ctx.pool).await?,
        1
    );

    let rejected = ctx.get(&path, Some(&token)).await?;
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(rejected.headers()["retry-after"], "1");
    let body: serde_json::Value = rejected.json().await?;
    assert_eq!(body["code"], "TOO_MANY_REQUESTS");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("close another stream"));

    drop(first.into_response());
    wait_for_no_leases(&ctx).await?;

    let replacement = ctx.get(&path, Some(&token)).await?;
    assert_eq!(replacement.status(), StatusCode::OK);
    drop(replacement.into_response());
    wait_for_no_leases(&ctx).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn route_emits_reconnect_event_and_releases_lease_on_shutdown() -> Result<()> {
    let ctx = TestContext::new_with_stream_limits(1, 1)
        .await?
        .with_auth()
        .await?;
    let (execution_id, token) = stream_fixture(&ctx).await?;
    let path = format!("/api/v1/executions/{execution_id}/logs/stdout/stream");
    let response = ctx.get(&path, Some(&token)).await?;
    assert_eq!(response.status(), StatusCode::OK);

    ctx.state.execution_log_streams.begin_shutdown();
    let body = tokio::time::timeout(Duration::from_secs(1), response.text()).await??;
    assert!(body.contains("event: error"));
    assert!(body.contains("server_shutting_down"));
    assert!(body.contains("reconnect using the last event ID"));
    wait_for_no_leases(&ctx).await?;
    Ok(())
}
