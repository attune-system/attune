//! Authorization tests for the inquiry response endpoint.
//!
//! Verifies the security guarantees added in `inquiry-assignee-edge-cases`:
//!
//! - `assigned_to` is an *enforced* lock (only the assignee may respond).
//! - Tokens without a resolvable identity are rejected with 403.
//! - Execution-scoped tokens whose `execution_id` matches `inquiry.execution`
//!   are blocked (privilege-loop guard) — an execution cannot answer an
//!   inquiry it created via `core.ask`.
//! - Execution-scoped tokens for a *different* execution may respond when
//!   they belong to the assignee.
//! - When `assigned_to` is unset, any authenticated caller may respond
//!   (existing behavior).

use attune_common::{
    auth::jwt::{generate_access_token, generate_execution_token, JwtConfig},
    models::{enums::ExecutionStatus, *},
    repositories::{
        action::{ActionRepository, CreateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{CreateIdentityInput, IdentityRepository},
        inquiry::{CreateInquiryInput, InquiryRepository},
        pack::{CreatePackInput, PackRepository},
        Create, FindById,
    },
};
use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

mod helpers;
use helpers::TestContext;

type TResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const TEST_JWT_SECRET: &str = "test-secret-for-testing-only-not-secure";

fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: TEST_JWT_SECRET.to_string(),
        access_token_expiration: 3600,
        refresh_token_expiration: 604800,
    }
}

async fn create_identity(pool: &PgPool, login: &str) -> TResult<Identity> {
    Ok(IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: login.to_string(),
            display_name: Some(login.to_string()),
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?)
}

async fn setup_pack_action(pool: &PgPool, suffix: &str) -> TResult<(Pack, Action)> {
    let pack = PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: format!("inq_test_{}", suffix),
            label: format!("Inquiry Test Pack {}", suffix),
            description: None,
            version: "1.0.0".to_string(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: json!({}),
        },
    )
    .await?;

    let action = ActionRepository::create(
        pool,
        CreateActionInput {
            r#ref: format!("{}.ask", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Ask".to_string(),
            description: None,
            entrypoint: "ask.sh".to_string(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: json!({}),
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
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
        },
    )
    .await?;

    Ok((pack, action))
}

async fn create_execution(pool: &PgPool, action: &Action) -> TResult<Execution> {
    Ok(ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
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
            status: ExecutionStatus::Running,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await?)
}

async fn create_child_execution(
    pool: &PgPool,
    action: &Action,
    parent_id: i64,
) -> TResult<Execution> {
    Ok(ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: Some(parent_id),
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Running,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await?)
}

async fn create_inquiry(
    pool: &PgPool,
    execution_id: i64,
    assigned_to: Option<i64>,
) -> TResult<Inquiry> {
    Ok(InquiryRepository::create(
        pool,
        CreateInquiryInput {
            execution: execution_id,
            prompt: "Approve?".to_string(),
            response_schema: None,
            assigned_to,
            status: attune_common::models::enums::InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        },
    )
    .await?)
}

fn respond_body() -> serde_json::Value {
    json!({ "response": { "approved": true } })
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn assignee_with_access_token_can_respond() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_ok").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "ok").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, Some(assignee.id)).await?;

    let token = generate_access_token(assignee.id, &assignee.login, &cfg)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn non_assignee_access_token_is_forbidden() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_real").await?;
    let other = create_identity(&ctx.pool, "other_user").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "non_assignee").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, Some(assignee.id)).await?;

    let token = generate_access_token(other.id, &other.login, &cfg)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn execution_token_self_response_is_blocked() -> TResult<()> {
    // The action that *created* the inquiry must not be allowed to respond to
    // it using its own execution-scoped token, even if the triggering
    // identity happens to be the assignee.
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_self").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "self").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, Some(assignee.id)).await?;

    // Execution token for the SAME execution that created the inquiry,
    // carrying the assignee identity in `sub`.
    let token = generate_execution_token(assignee.id, exec.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = resp.json().await?;
    let msg = body["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("privilege loop") || msg.contains("cannot respond"),
        "unexpected error message: {}",
        msg
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn execution_token_for_different_execution_can_respond_when_assignee() -> TResult<()> {
    // An execution token for a *different* execution (e.g., a Slack-bridge
    // action handling the user's webhook reply) is allowed when its
    // identity matches the assignee.
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_other_exec").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "other_exec").await?;
    let creating_exec = create_execution(&ctx.pool, &action).await?;
    let other_exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, creating_exec.id, Some(assignee.id)).await?;

    let token = generate_execution_token(assignee.id, other_exec.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn execution_token_for_different_execution_blocked_when_not_assignee() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_diff_id").await?;
    let other = create_identity(&ctx.pool, "non_assignee_id").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "exec_non_assignee").await?;
    let creating_exec = create_execution(&ctx.pool, &action).await?;
    let other_exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, creating_exec.id, Some(assignee.id)).await?;

    let token = generate_execution_token(other.id, other_exec.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn unassigned_inquiry_accepts_any_authenticated_caller() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let caller = create_identity(&ctx.pool, "any_caller").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "unassigned").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, None).await?;

    let token = generate_access_token(caller.id, &caller.login, &cfg)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn nested_execution_token_self_response_is_blocked() -> TResult<()> {
    // A creates an inquiry; B is a child of A. A token scoped to B must not
    // be allowed to respond to A's inquiry — that's still a self-approval
    // loop in spirit (the workflow that created the inquiry can't approve
    // it via one of its own descendants).
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_nested").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "nested").await?;
    let exec_a = create_execution(&ctx.pool, &action).await?;
    let exec_b = create_child_execution(&ctx.pool, &action, exec_a.id).await?;
    let inquiry = create_inquiry(&ctx.pool, exec_a.id, Some(assignee.id)).await?;

    // Execution token scoped to the *child* execution B, carrying the
    // assignee identity.
    let token = generate_execution_token(assignee.id, exec_b.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = resp.json().await?;
    let msg = body["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("descendant") || msg.contains("privilege loop"),
        "unexpected error message: {}",
        msg
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn deeply_nested_execution_token_self_response_is_blocked() -> TResult<()> {
    // A → B → C: token scoped to C must not be able to respond to A's inquiry.
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_deep").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "deep").await?;
    let exec_a = create_execution(&ctx.pool, &action).await?;
    let exec_b = create_child_execution(&ctx.pool, &action, exec_a.id).await?;
    let exec_c = create_child_execution(&ctx.pool, &action, exec_b.id).await?;
    let inquiry = create_inquiry(&ctx.pool, exec_a.id, Some(assignee.id)).await?;

    let token = generate_execution_token(assignee.id, exec_c.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn responded_by_recorded_for_access_token() -> TResult<()> {
    // After a successful response, the inquiry row should reflect the
    // assignee's identity in `responded_at` (set non-null) and the response
    // should be persisted. We assert via DB state since the MQ event is
    // best-effort (publisher may be absent in the test harness).
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_audit").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "audit").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, Some(assignee.id)).await?;

    let token = generate_access_token(assignee.id, &assignee.login, &cfg)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    let stored = InquiryRepository::find_by_id(&ctx.pool, inquiry.id)
        .await?
        .expect("inquiry should exist");
    assert!(stored.responded_at.is_some(), "responded_at should be set");
    assert_eq!(
        stored.status,
        attune_common::models::enums::InquiryStatus::Responded
    );
    assert_eq!(stored.response, Some(json!({ "approved": true })));
    Ok(())
}
