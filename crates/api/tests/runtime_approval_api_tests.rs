mod helpers;

use attune_api::authz::AuthorizationService;
use attune_common::repositories::{
    identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    },
    Create,
};
use helpers::{create_test_pack, TestContext};
use reqwest::StatusCode;
use serde_json::{json, Value};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

async fn shared_target(ctx: &TestContext) -> Result<()> {
    let shared = create_test_pack(&ctx.pool, "shared").await?;
    attune_common::repositories::trigger::TriggerRepository::create(
        &ctx.pool,
        attune_common::repositories::trigger::CreateTriggerInput {
            r#ref: "shared.completed".into(),
            pack: Some(shared.id),
            pack_ref: Some(shared.r#ref),
            label: "Shared target".into(),
            description: None,
            enabled: true,
            param_schema: None,
            event_config_schema: None,
            out_schema: None,
            sensor: None,
            sensor_ref: None,
            is_adhoc: false,
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await?;
    Ok(())
}

async fn issuer() -> Result<TestContext> {
    let ctx = TestContext::new().await?.with_auth().await?;
    shared_target(&ctx).await?;
    let identity = ctx.user.as_ref().unwrap().id;
    let set = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.approval_issuer".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource": "packs", "actions": ["read"]},
            {"resource": "events", "actions": ["create"], "constraints": {"refs": ["shared.completed"]}}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity).await;
    Ok(ctx)
}

fn grant() -> Value {
    json!({"resource": "events", "actions": ["create"], "constraints": {"refs": ["shared.completed"]}})
}

#[tokio::test]
async fn owner_can_issue_inspect_replace_and_revoke_fixed_external_approvals() -> Result<()> {
    let ctx = issuer().await?;
    create_test_pack(&ctx.pool, "approval_subject").await?;
    let path = "/api/v1/packs/approval_subject/external-approvals";
    let response = ctx
        .post(path, json!({"grants": [grant()]}), ctx.token())
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["data"]["id"].as_i64().unwrap();
    assert_eq!(body["data"]["active"], true);
    assert_eq!(
        body["data"]["issuer_identity_id"],
        ctx.user.as_ref().unwrap().id
    );
    let response = ctx.get(&format!("{path}/{id}"), ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    let response = ctx
        .post(
            &format!("{path}/{id}/replace"),
            json!({"grants": [grant()], "expires_at": expires}),
            ctx.token(),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let replacement = body["data"]["id"].as_i64().unwrap();
    assert_ne!(id, replacement);
    let previous: Value = ctx
        .get(&format!("{path}/{id}"), ctx.token())
        .await?
        .json()
        .await?;
    assert_eq!(previous["data"]["active"], false);
    assert!(!previous["data"]["revoked_at"].is_null());
    let listed: Value = ctx.get(path, ctx.token()).await?.json().await?;
    assert_eq!(listed["items"].as_array().unwrap().len(), 2);
    for _ in 0..2 {
        let response = ctx
            .post(
                &format!("{path}/{replacement}/revoke"),
                json!({}),
                ctx.token(),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert_eq!(body["data"]["active"], false);
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn pack_read_does_not_authorize_broader_external_grants_or_cross_subject_controls(
) -> Result<()> {
    let ctx = issuer().await?;
    create_test_pack(&ctx.pool, "first_subject").await?;
    create_test_pack(&ctx.pool, "second_subject").await?;
    let path = "/api/v1/packs/first_subject/external-approvals";
    let denied = ctx
        .post(
            path,
            json!({"grants": [{"resource": "events", "actions": ["create"]}]}),
            ctx.token(),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let issued: Value = ctx
        .post(path, json!({"grants": [grant()]}), ctx.token())
        .await?
        .json()
        .await?;
    let id = issued["data"]["id"].as_i64().unwrap();
    let wrong = format!("/api/v1/packs/second_subject/external-approvals/{id}/revoke");
    assert_eq!(
        ctx.post(&wrong, json!({}), ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    let expired = ctx
        .post(
            path,
            json!({"grants": [grant()], "expires_at": "2000-01-01T00:00:00Z"}),
            ctx.token(),
        )
        .await?;
    assert!(expired.status().is_client_error());
    let listed: Value = ctx.get(path, ctx.token()).await?.json().await?;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn permission_administrator_can_approve_without_personally_holding_the_runtime_grant(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    shared_target(&ctx).await?;
    create_test_pack(&ctx.pool, "admin_subject").await?;
    let response = ctx.post("/api/v1/packs/admin_subject/external-approvals",
        json!({"grants": [{"resource": "events", "actions": ["create"], "constraints": {"refs": ["shared.completed"]}}]}),
        ctx.token()).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["active"], true);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn approval_endpoints_require_authenticated_pack_inspection_authority() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    create_test_pack(&ctx.pool, "private_subject").await?;
    let path = "/api/v1/packs/private_subject/external-approvals";
    assert_eq!(
        // None uses the context's authenticated token. Override it explicitly.
        ctx.get(path, Some("invalid-token")).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        ctx.get(path, ctx.token()).await?.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.post(path, json!({"grants": [grant()]}), ctx.token())
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    ctx.cleanup().await?;
    Ok(())
}
