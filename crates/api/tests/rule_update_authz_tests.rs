mod helpers;

use attune_common::{
    auth::jwt::{
        generate_execution_token, generate_execution_token_with_permission_sets,
        generate_worker_token, JwtConfig,
    },
    repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        rule::{CreateRuleInput, RuleRepository},
        Create, FindByRef,
    },
};
use axum::http::StatusCode;
use helpers::{create_test_action, create_test_pack, create_test_trigger, Result, TestContext};
use serde_json::json;

const TEST_JWT_SECRET: &str = "test-secret-for-testing-only-not-secure";

fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: TEST_JWT_SECRET.to_string(),
        access_token_expiration: 3600,
        refresh_token_expiration: 604800,
    }
}

async fn seed_rule_and_identity(ctx: &TestContext, suffix: &str) -> Result<(String, i64)> {
    let pack = create_test_pack(&ctx.pool, &format!("rule_authz_{suffix}")).await?;
    let action_ref = format!("{}.action", pack.r#ref);
    let trigger_ref = format!("{}.trigger", pack.r#ref);
    let action = create_test_action(&ctx.pool, pack.id, &pack.r#ref, &action_ref).await?;
    let trigger = create_test_trigger(&ctx.pool, pack.id, &trigger_ref).await?;
    let rule_ref = format!("{}.rule", pack.r#ref);
    RuleRepository::create(
        &ctx.pool,
        CreateRuleInput {
            r#ref: rule_ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref,
            label: "Rule update authorization test".to_string(),
            description: None,
            action: action.id,
            action_ref,
            trigger: trigger.id,
            trigger_ref,
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: true,
            owner_identity: None,
        },
    )
    .await?;

    let identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: format!("rule_authz_{suffix}"),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;

    Ok((rule_ref, identity.id))
}

#[tokio::test]
async fn rule_writes_store_key_templates_and_reject_literal_secret_parameters() -> Result<()> {
    use attune_common::auth::jwt::{generate_token, TokenType};
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        trigger::{TriggerRepository, UpdateTriggerInput},
        Patch,
    };
    let ctx = TestContext::new().await?;
    let (rule_ref, identity_id) = seed_rule_and_identity(&ctx, "key_templates").await?;
    let rule = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
        .await?
        .unwrap();
    let schema = json!({"credential":{"type":"string","secret":true},
        "count":{"type":"integer","required":true,"default":5}});
    ActionRepository::update(
        &ctx.pool,
        rule.action.unwrap(),
        UpdateActionInput {
            param_schema: Some(schema.clone()),
            ..Default::default()
        },
    )
    .await?;
    TriggerRepository::update(
        &ctx.pool,
        rule.trigger.unwrap(),
        UpdateTriggerInput {
            param_schema: Some(Patch::Set(schema)),
            ..Default::default()
        },
    )
    .await?;
    let permission = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "test.rule_key_templates".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([
                {"resource":"rules","actions":["read","create","update"]},
                {"resource":"actions","actions":["read","execute"]},
                {"resource":"triggers","actions":["read"]},
                {"resource":"packs","actions":["read"]}
            ]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity_id,
            permset: permission.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let token = generate_token(
        identity_id,
        "rule_authz_key_templates",
        &jwt_config(),
        TokenType::Access,
    )?;
    for field in ["action_params", "trigger_params"] {
        let response = ctx
            .put(
                &format!("/api/v1/rules/{rule_ref}"),
                json!({(field):{"credential":"private-rule-literal"}}),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: serde_json::Value = response.json().await?;
        assert!(!body.to_string().contains("private-rule-literal"));
        let unchanged = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
            .await?
            .unwrap();
        assert_eq!(unchanged.action_params, json!({}));
        assert_eq!(unchanged.trigger_params, json!({}));
    }
    let selector = json!({"credential":"{{ keystore['system.api_token'] }}"});
    ctx.put(
        &format!("/api/v1/rules/{rule_ref}"),
        json!({"action_params":selector,"trigger_params":selector}),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);
    let stored = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
        .await?
        .unwrap();
    assert_eq!(stored.action_params, selector);
    assert_eq!(stored.trigger_params, selector);
    // Required defaults are validated without materializing them into the
    // authored selectors. Runtime admission owns their immutable resolved copy.
    let defaulted_ref = format!("{}.defaulted_rule", rule.pack_ref);
    ctx.post(
        "/api/v1/rules",
        json!({"ref":defaulted_ref,"label":"Defaulted Rule","pack_ref":rule.pack_ref,
        "trigger_ref":rule.trigger_ref,"action_ref":rule.action_ref,
        "trigger_params":selector,"action_params":selector}),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::CREATED);
    let defaulted = RuleRepository::find_by_ref(&ctx.pool, &defaulted_ref)
        .await?
        .unwrap();
    assert_eq!(defaulted.action_params, selector);
    assert_eq!(defaulted.trigger_params, selector);
    let rejected_ref = format!("{}.rejected_secret", rule.pack_ref);
    ctx.post(
        "/api/v1/rules",
        json!({"ref":rejected_ref,"label":"Rejected Secret Rule","pack_ref":rule.pack_ref,
        "trigger_ref":rule.trigger_ref,"action_ref":rule.action_ref,
        "trigger_params":{"credential":"private-rule-literal"},"action_params":selector}),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert!(RuleRepository::find_by_ref(&ctx.pool, &rejected_ref)
        .await?
        .is_none());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn unauthorized_execution_and_worker_tokens_cannot_update_rule_trace_tag_template(
) -> Result<()> {
    let ctx = TestContext::new().await?;
    let (rule_ref, identity_id) = seed_rule_and_identity(&ctx, "denied").await?;
    let tokens = [
        generate_execution_token(identity_id, 1, "test.action", &jwt_config(), Some(600))?,
        generate_worker_token(identity_id, "test-worker", &jwt_config(), Some(600))?,
    ];

    for token in tokens {
        let response = ctx
            .put(
                &format!("/api/v1/rules/{rule_ref}"),
                json!({ "trace_tag_template": "unauthorized-change" }),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let rule = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
            .await?
            .expect("seeded rule");
        assert_eq!(rule.trace_tag_template, None);
    }

    Ok(())
}

#[tokio::test]
async fn execution_token_with_rules_update_permission_can_update_trace_tag_template() -> Result<()>
{
    let ctx = TestContext::new().await?;
    let (rule_ref, identity_id) = seed_rule_and_identity(&ctx, "allowed").await?;
    let permission_set_ref = "test.rule_update";
    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: permission_set_ref.to_string(),
            pack: None,
            pack_ref: None,
            label: Some("Rule update".to_string()),
            description: None,
            grants: json!([{ "resource": "rules", "actions": ["update"] }]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity_id,
            permset: permission_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let token = generate_execution_token_with_permission_sets(
        identity_id,
        2,
        "test.action",
        &jwt_config(),
        Some(600),
        &[permission_set_ref.to_string()],
    )?;

    let response = ctx
        .put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({ "trace_tag_template": "authorized-change" }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let rule = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
        .await?
        .expect("seeded rule");
    assert_eq!(
        rule.trace_tag_template.as_deref(),
        Some("authorized-change")
    );

    ctx.cleanup().await?;
    Ok(())
}
