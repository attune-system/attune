//! Authorization tests for the inquiry response endpoint.
//!
//! Verifies the security guarantees added in `inquiry-assignee-edge-cases`:
//!
//! - `assigned_to` is an *enforced* lock (only the assignee may respond).
//! - Tokens without a resolvable identity are rejected with 403.
//! - Execution-scoped tokens whose `execution_id` matches `inquiry.execution`
//!   are blocked (privilege-loop guard) — an execution cannot answer an
//!   inquiry it created.
//! - Execution-scoped tokens for a *different* execution may respond when
//!   they belong to the assignee.
//! - When `assigned_to` is unset, any authenticated caller may respond
//!   (existing behavior).

use attune_common::{
    auth::jwt::{
        generate_access_token, generate_execution_token, generate_integration_access_token,
        JwtConfig,
    },
    inquiry_response_handle::issue_inquiry_response_handle,
    models::{enums::ExecutionStatus, *},
    repositories::{
        action::{ActionRepository, CreateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        external_identity_mapping::{
            CreateExternalIdentityMappingInput, ExternalIdentityMappingRepository,
        },
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
            UpdateIdentityInput,
        },
        inquiry::{CreateInquiryInput, InquiryRepository},
        integration_token::{CreateIntegrationTokenInput, IntegrationTokenRepository},
        pack::{CreatePackInput, PackRepository},
        Create, Delete, FindById, Update,
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
            trace_tag: None,
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
            trace_tag: None,
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

fn response_handle(ctx: &TestContext, inquiry_id: i64) -> String {
    issue_inquiry_response_handle(
        inquiry_id,
        ctx.state
            .config
            .security
            .encryption_key
            .as_deref()
            .expect("test encryption key"),
    )
    .expect("response handle")
}

fn external_respond_body(response_handle: &str) -> serde_json::Value {
    json!({
        "response_handle": response_handle,
        "external_actor": {
            "provider": " GitHub ",
            "tenant": " Acme ",
            "external_subject": " User-42 "
        },
        "response": { "approved": true }
    })
}

struct ExternalResponseFixture {
    token: String,
    integration_token_id: i64,
    integration_identity_id: i64,
    permission_assignment_id: i64,
    mapped_identity_id: i64,
    mapping_id: i64,
    inquiry_id: i64,
    response_handle: String,
}

async fn setup_external_response_fixture(
    ctx: &TestContext,
    suffix: &str,
) -> TResult<ExternalResponseFixture> {
    setup_external_response_fixture_with_expiry(ctx, suffix, None).await
}

async fn setup_external_response_fixture_with_expiry(
    ctx: &TestContext,
    suffix: &str,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> TResult<ExternalResponseFixture> {
    let integration = create_identity(&ctx.pool, &format!("provider_integration_{suffix}")).await?;
    let mapped = create_identity(&ctx.pool, &format!("provider_mapped_{suffix}")).await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, &format!("provider_{suffix}")).await?;
    let execution = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, execution.id, Some(mapped.id)).await?;

    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.provider_inquiry_{suffix}"),
            pack: None,
            pack_ref: None,
            label: Some("Provider inquiry response".to_string()),
            description: None,
            grants: json!([{"resource": "inquiries", "actions": ["respond"]}]),
        },
    )
    .await?;
    let permission_assignment = PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: integration.id,
            permset: permission_set.id,
        },
    )
    .await?;
    let integration_token = IntegrationTokenRepository::create(
        &ctx.pool,
        CreateIntegrationTokenInput {
            identity: integration.id,
            label: format!("Provider {suffix}"),
            description: None,
            token_hash: format!("provider-hash-{suffix}"),
            token_prefix: "attune_it_provider".to_string(),
            token_suffix: suffix.to_string(),
            created_by: Some(integration.id),
            expires_at,
        },
    )
    .await?;
    let mapping = ExternalIdentityMappingRepository::create(
        &ctx.pool,
        integration.id,
        CreateExternalIdentityMappingInput {
            mapped_identity: mapped.id,
            provider: "github".to_string(),
            tenant: "Acme".to_string(),
            external_subject: "User-42".to_string(),
            created_by: Some(integration.id),
        },
    )
    .await?;
    let token = generate_integration_access_token(
        integration.id,
        integration_token.id,
        &integration.login,
        &jwt_config(),
    )?;

    Ok(ExternalResponseFixture {
        token,
        integration_token_id: integration_token.id,
        integration_identity_id: integration.id,
        permission_assignment_id: permission_assignment.id,
        mapped_identity_id: mapped.id,
        mapping_id: mapping.id,
        inquiry_id: inquiry.id,
        response_handle: response_handle(ctx, inquiry.id),
    })
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
    assert_eq!(stored.responded_by, Some(assignee.id));
    assert_eq!(stored.response, Some(json!({ "approved": true })));
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn external_response_rejects_non_integration_access_and_workload_tokens() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let caller = create_identity(&ctx.pool, "provider_wrong_token").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "provider_wrong_token").await?;
    let execution = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, execution.id, Some(caller.id)).await?;
    let access_token = generate_access_token(caller.id, &caller.login, &jwt_config())?;
    let execution_token =
        generate_execution_token(caller.id, execution.id, &action.r#ref, &jwt_config(), None)?;

    for token in [access_token, execution_token] {
        let response = ctx
            .post(
                "/api/v1/inquiry-responses",
                external_respond_body(&response_handle(&ctx, inquiry.id)),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn external_response_records_only_allowlisted_provenance() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_external_response_fixture(&ctx, "success").await?;

    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            external_respond_body(&fixture.response_handle),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let stored = InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
        .await?
        .expect("inquiry should exist");
    assert_eq!(stored.responded_by, Some(fixture.mapped_identity_id));
    assert_eq!(stored.response, Some(json!({"approved": true})));
    assert_eq!(
        stored.external_actor,
        Some(json!({
            "provider": "github",
            "tenant": "Acme",
            "external_subject": "User-42",
            "mapping_id": fixture.mapping_id,
            "integration_identity_id": fixture.integration_identity_id,
            "integration_token_id": fixture.integration_token_id,
        }))
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn external_response_revalidates_revoked_credential() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_external_response_fixture(&ctx, "revoked").await?;
    IntegrationTokenRepository::revoke(
        &ctx.pool,
        fixture.integration_token_id,
        Some(fixture.integration_identity_id),
        Some("test revocation"),
    )
    .await?;

    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            external_respond_body(&fixture.response_handle),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let stored = InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
        .await?
        .expect("inquiry should exist");
    assert_eq!(
        stored.status,
        attune_common::models::enums::InquiryStatus::Pending
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn external_response_rejects_tampered_handle_without_disclosing_target() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_external_response_fixture(&ctx, "tampered_handle").await?;
    let mut tampered = fixture.response_handle.clone();
    tampered.push('x');

    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            external_respond_body(&tampered),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.text().await?;
    assert!(!body.contains(&fixture.response_handle));
    assert!(!body.contains(&fixture.inquiry_id.to_string()));
    assert_eq!(
        InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
            .await?
            .expect("inquiry should exist")
            .status,
        InquiryStatus::Pending
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn external_response_rejects_expired_and_frozen_identities() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let expired = setup_external_response_fixture_with_expiry(
        &ctx,
        "expired",
        Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
    )
    .await?;
    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            external_respond_body(&expired.response_handle),
            Some(&expired.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let frozen_integration = setup_external_response_fixture(&ctx, "frozen_integration").await?;
    IdentityRepository::update(
        &ctx.pool,
        frozen_integration.integration_identity_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            external_respond_body(&frozen_integration.response_handle),
            Some(&frozen_integration.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let frozen_mapped = setup_external_response_fixture(&ctx, "frozen_mapped").await?;
    IdentityRepository::update(
        &ctx.pool,
        frozen_mapped.mapped_identity_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            external_respond_body(&frozen_mapped.response_handle),
            Some(&frozen_mapped.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn external_response_rejects_mapping_assignment_and_schema_mismatches() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_external_response_fixture(&ctx, "mismatches").await?;
    let original = InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
        .await?
        .expect("fixture inquiry");

    let mut wrong_tenant = external_respond_body(&fixture.response_handle);
    wrong_tenant["external_actor"]["tenant"] = json!("Other-Team");
    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            wrong_tenant,
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    for assigned_to in [
        None,
        Some(create_identity(&ctx.pool, "wrong_assignee").await?.id),
    ] {
        let inquiry = create_inquiry(&ctx.pool, original.execution, assigned_to).await?;
        let response = ctx
            .post(
                "/api/v1/inquiry-responses",
                external_respond_body(&response_handle(&ctx, inquiry.id)),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    let schema_inquiry = InquiryRepository::create(
        &ctx.pool,
        CreateInquiryInput {
            execution: original.execution,
            prompt: "Approve?".to_string(),
            response_schema: Some(json!({
                "approved": {"type": "boolean", "required": true}
            })),
            assigned_to: Some(fixture.mapped_identity_id),
            status: InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        },
    )
    .await?;
    let mut invalid_response = external_respond_body(&response_handle(&ctx, schema_inquiry.id));
    invalid_response["response"] = json!({"approved": "yes"});
    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            invalid_response,
            Some(&fixture.token),
        )
        .await?;
    assert!(response.status().is_client_error());
    assert_eq!(
        InquiryRepository::find_by_id(&ctx.pool, schema_inquiry.id)
            .await?
            .expect("schema inquiry")
            .status,
        InquiryStatus::Pending
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn external_response_uses_current_database_authorization() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_external_response_fixture(&ctx, "rbac_revoked").await?;
    assert!(
        PermissionAssignmentRepository::delete(&ctx.pool, fixture.permission_assignment_id).await?
    );

    let response = ctx
        .post(
            "/api/v1/inquiry-responses",
            external_respond_body(&fixture.response_handle),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let stored = InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
        .await?
        .expect("inquiry should exist");
    assert_eq!(
        stored.status,
        attune_common::models::enums::InquiryStatus::Pending
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn external_response_rejects_caller_supplied_actor_evidence() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_external_response_fixture(&ctx, "extra_evidence").await?;
    let mut body = external_respond_body(&fixture.response_handle);
    body["responded_by"] = json!(fixture.mapped_identity_id);
    body["evidence"] = json!({"callback": "untrusted"});

    let response = ctx
        .post("/api/v1/inquiry-responses", body, Some(&fixture.token))
        .await?;
    assert!(response.status().is_client_error());
    let stored = InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
        .await?
        .expect("inquiry should exist");
    assert_eq!(
        stored.status,
        attune_common::models::enums::InquiryStatus::Pending
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn concurrent_external_responses_have_one_winner() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_external_response_fixture(&ctx, "concurrent").await?;
    let path = "/api/v1/inquiry-responses";

    let first = ctx.post(
        path,
        external_respond_body(&fixture.response_handle),
        Some(&fixture.token),
    );
    let second = ctx.post(
        path,
        external_respond_body(&fixture.response_handle),
        Some(&fixture.token),
    );
    let (first, second) = tokio::join!(first, second);
    let statuses = [first?.status(), second?.status()];

    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );

    let stored = InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
        .await?
        .expect("inquiry should exist");
    assert_eq!(stored.responded_by, Some(fixture.mapped_identity_id));
    assert_eq!(stored.status, InquiryStatus::Responded);
    Ok(())
}
