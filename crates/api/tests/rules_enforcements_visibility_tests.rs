//! Row-level visibility tests for the rules and enforcements read APIs.
//!
//! These verify the private-scoped rule model and rule-derived authoritative
//! enforcement model described in `docs/permissions/permissions-high-level.md`.

use axum::http::StatusCode;
use helpers::*;
use serde_json::json;

use attune_common::{
    models::enums::{EnforcementCondition, EnforcementStatus},
    repositories::{
        event::{CreateEnforcementInput, EnforcementRepository},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        pack::PackRepository,
        rule::{CreateRuleInput, RuleRepository},
        Create, FindByRef,
    },
};

mod helpers;

async fn register_scoped_user(
    ctx: &TestContext,
    login: &str,
    grants: serde_json::Value,
) -> Result<String> {
    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": login,
                "password": "TestPassword123!",
                "display_name": format!("Scoped User {}", login),
            }),
            None,
        )
        .await?;

    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::CREATED,
        "expected 200/201 from /auth/register, got {}",
        response.status()
    );
    let body: serde_json::Value = response.json().await?;
    let token = body["data"]["access_token"]
        .as_str()
        .expect("missing access token")
        .to_string();

    if !grants.as_array().map(|a| a.is_empty()).unwrap_or(true) {
        let identity = IdentityRepository::find_by_login(&ctx.pool, login)
            .await?
            .expect("registered identity should exist");

        let permset = PermissionSetRepository::create(
            &ctx.pool,
            CreatePermissionSetInput {
                r#ref: format!("test.scoped_{}", uuid::Uuid::new_v4().simple()),
                pack: None,
                pack_ref: None,
                label: Some("Scoped Test Permission Set".to_string()),
                description: Some("Scoped test grants".to_string()),
                grants,
            },
        )
        .await?;

        PermissionAssignmentRepository::create(
            &ctx.pool,
            CreatePermissionAssignmentInput {
                identity: identity.id,
                permset: permset.id,
            },
        )
        .await?;

        attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
        attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    }

    Ok(token)
}

struct RuleFixture {
    id: i64,
    r#ref: String,
    action_ref: String,
    trigger_ref: String,
}

async fn seed_rule(ctx: &TestContext, pack_ref: &str, rule_name: &str) -> Result<RuleFixture> {
    let pack = match PackRepository::find_by_ref(&ctx.pool, pack_ref).await? {
        Some(pack) => pack,
        None => create_test_pack(&ctx.pool, pack_ref).await?,
    };
    let action_ref = format!("{pack_ref}.{rule_name}_act");
    let trigger_ref = format!("{pack_ref}.{rule_name}_trg");
    let action = create_test_action(&ctx.pool, pack.id, &pack.r#ref, &action_ref).await?;
    let trigger = create_test_trigger(&ctx.pool, pack.id, &trigger_ref).await?;

    let rule_ref = format!("{pack_ref}.{rule_name}");
    let rule = RuleRepository::create(
        &ctx.pool,
        CreateRuleInput {
            r#ref: rule_ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: None,
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await?;

    Ok(RuleFixture {
        id: rule.id,
        r#ref: rule.r#ref,
        action_ref: rule.action_ref,
        trigger_ref: rule.trigger_ref,
    })
}

async fn seed_enforcement(ctx: &TestContext, rule: &RuleFixture) -> Result<i64> {
    let enforcement = EnforcementRepository::create(
        &ctx.pool,
        CreateEnforcementInput {
            rule: Some(rule.id),
            rule_ref: rule.r#ref.clone(),
            trigger_ref: rule.trigger_ref.clone(),
            config: None,
            event: None,
            status: EnforcementStatus::Created,
            payload: json!({}),
            condition: EnforcementCondition::All,
            conditions: json!([]),
        },
    )
    .await?;
    Ok(enforcement.id)
}

async fn list_rule_refs(ctx: &TestContext, path: &str, token: &str) -> Result<Vec<String>> {
    let response = ctx.get(path, Some(token)).await?;
    assert_eq!(response.status(), StatusCode::OK, "GET {path} should be 200");
    let body: serde_json::Value = response.json().await?;
    Ok(body["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|item| item["ref"].as_str().unwrap_or_default().to_string())
        .collect())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_rules_hidden_without_read_grant() {
    let ctx = TestContext::new().await.unwrap();
    let rule = seed_rule(&ctx, "packa", "rule_one").await.unwrap();

    let token = register_scoped_user(&ctx, "no_grants_user", json!([]))
        .await
        .unwrap();

    let refs = list_rule_refs(&ctx, "/api/v1/rules", &token).await.unwrap();
    assert!(
        refs.is_empty(),
        "user with no rules:read grant must see no rules, saw {refs:?}"
    );

    // Enabled + per-pack + per-trigger + per-action endpoints all deny too.
    for path in [
        "/api/v1/rules/enabled".to_string(),
        "/api/v1/packs/packa/rules".to_string(),
        format!("/api/v1/triggers/{}/rules", rule.trigger_ref),
        format!("/api/v1/actions/{}/rules", rule.action_ref),
    ] {
        let refs = list_rule_refs(&ctx, &path, &token).await.unwrap();
        assert!(refs.is_empty(), "{path} must be empty, saw {refs:?}");
    }

    // Single-rule get must not leak existence.
    let response = ctx
        .get("/api/v1/rules/packa.rule_one", Some(&token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_global_rules_read_returns_all() {
    let ctx = TestContext::new().await.unwrap();
    seed_rule(&ctx, "packa", "rule_one").await.unwrap();
    seed_rule(&ctx, "packb", "rule_two").await.unwrap();

    let token = register_scoped_user(
        &ctx,
        "global_rules_user",
        json!([{ "resource": "rules", "actions": ["read"] }]),
    )
    .await
    .unwrap();

    let mut refs = list_rule_refs(&ctx, "/api/v1/rules", &token).await.unwrap();
    refs.sort();
    assert_eq!(refs, vec!["packa.rule_one", "packb.rule_two"]);

    let response = ctx
        .get("/api/v1/rules/packa.rule_one", Some(&token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_scoped_rules_read_filters_rows() {
    let ctx = TestContext::new().await.unwrap();
    seed_rule(&ctx, "packa", "rule_one").await.unwrap();
    seed_rule(&ctx, "packb", "rule_two").await.unwrap();

    let token = register_scoped_user(
        &ctx,
        "pack_scoped_user",
        json!([{
            "resource": "rules",
            "actions": ["read"],
            "constraints": { "pack_refs": ["packa"] }
        }]),
    )
    .await
    .unwrap();

    let refs = list_rule_refs(&ctx, "/api/v1/rules", &token).await.unwrap();
    assert_eq!(refs, vec!["packa.rule_one"]);

    // Allowed pack rule is readable; other pack's rule is hidden as 404.
    assert_eq!(
        ctx.get("/api/v1/rules/packa.rule_one", Some(&token))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.get("/api/v1/rules/packb.rule_two", Some(&token))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_specific_rule_read_filters_rows() {
    let ctx = TestContext::new().await.unwrap();
    seed_rule(&ctx, "packa", "rule_one").await.unwrap();
    seed_rule(&ctx, "packa", "rule_two").await.unwrap();

    let token = register_scoped_user(
        &ctx,
        "specific_rule_user",
        json!([{
            "resource": "rules",
            "actions": ["read"],
            "constraints": { "refs": ["packa.rule_one"] }
        }]),
    )
    .await
    .unwrap();

    let refs = list_rule_refs(&ctx, "/api/v1/rules", &token).await.unwrap();
    assert_eq!(refs, vec!["packa.rule_one"]);
    assert_eq!(
        ctx.get("/api/v1/rules/packa.rule_two", Some(&token))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enforcements_require_readable_rule() {
    let ctx = TestContext::new().await.unwrap();
    let rule_a = seed_rule(&ctx, "packa", "rule_one").await.unwrap();
    let rule_b = seed_rule(&ctx, "packb", "rule_two").await.unwrap();
    seed_enforcement(&ctx, &rule_a).await.unwrap();
    seed_enforcement(&ctx, &rule_b).await.unwrap();

    // No rule read grant => rule-derived visibility denies all enforcements.
    let no_grant = register_scoped_user(&ctx, "enf_no_grant", json!([]))
        .await
        .unwrap();
    let response = ctx
        .get("/api/v1/enforcements", Some(&no_grant))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["items"].as_array().unwrap().is_empty(),
        "enforcements must be hidden without readable rule, saw {body}"
    );

    // Pack-scoped rule read => only enforcements for that rule.
    let pack_scoped = register_scoped_user(
        &ctx,
        "enf_pack_scoped",
        json!([{
            "resource": "rules",
            "actions": ["read"],
            "constraints": { "pack_refs": ["packa"] }
        }]),
    )
    .await
    .unwrap();
    let body: serde_json::Value = ctx
        .get("/api/v1/enforcements", Some(&pack_scoped))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rule_refs: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["rule_ref"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(rule_refs, vec!["packa.rule_one"]);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_global_enforcement_read_returns_all() {
    let ctx = TestContext::new().await.unwrap();
    let rule_a = seed_rule(&ctx, "packa", "rule_one").await.unwrap();
    let rule_b = seed_rule(&ctx, "packb", "rule_two").await.unwrap();
    seed_enforcement(&ctx, &rule_a).await.unwrap();
    seed_enforcement(&ctx, &rule_b).await.unwrap();

    let token = register_scoped_user(
        &ctx,
        "global_enf_user",
        json!([{ "resource": "enforcements", "actions": ["read"] }]),
    )
    .await
    .unwrap();

    let body: serde_json::Value = ctx
        .get("/api/v1/enforcements", Some(&token))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut rule_refs: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["rule_ref"].as_str().unwrap_or_default().to_string())
        .collect();
    rule_refs.sort();
    assert_eq!(rule_refs, vec!["packa.rule_one", "packb.rule_two"]);
}
