mod helpers;

use attune_common::{
    auth::jwt::{generate_worker_token, JwtConfig},
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
                {"resource":"packs","actions":["read"]},
                {"resource":"keys","actions":["read","decrypt"]}
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
    seed_input_key(&ctx).await?;
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
        helpers::delegated_source::token(&ctx, identity_id, &["standard".into()]).await?,
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
            grants: json!([{ "resource": "rules", "actions": ["update"] }, {"resource":"actions","actions":["execute"]}]),
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
    let token =
        helpers::delegated_source::token(&ctx, identity_id, &[permission_set_ref.to_string()])
            .await?;

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

async fn seed_input_key(ctx: &TestContext) -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::key::{CreateKeyInput, KeyRepository},
    };
    KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "api_token".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Input fixture".into(),
            encrypted: true,
            encryption_key_hash: Some("fixture-unusable".into()),
            // Intentionally not decryptable: write admission must inspect metadata only.
            value: json!("not-ciphertext"),
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn behavioral_edits_require_actual_editor_authority_even_for_same_subject_tokens(
) -> Result<()> {
    use attune_common::{
        auth::jwt::{generate_token, TokenType},
        repositories::{rule::UpdateRuleInput, Patch, Update},
    };
    let ctx = TestContext::new().await?;
    let (rule_ref, owner) = seed_rule_and_identity(&ctx, "sponsor").await?;
    let rule = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
        .await?
        .unwrap();
    seed_input_key(&ctx).await?;
    let privileged = create_test_action(
        &ctx.pool,
        rule.pack,
        &rule.pack_ref,
        &format!("{}.privileged", rule.pack_ref),
    )
    .await?;
    let admin = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "test.sponsor_admin".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([
                {"resource":"rules","actions":["update"]},
                {"resource":"actions","actions":["execute"]},
                {"resource":"keys","actions":["read","decrypt"]},
                {"resource":"packs","actions":["read","decrypt"]}
            ]),
        },
    )
    .await?;
    let narrow = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "test.narrow_editor".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource":"rules","actions":["update"]},
            {"resource":"actions","actions":["execute"],"constraints":{"refs":[rule.action_ref]}}
        ]),
    }).await?;
    let editor = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "different_editor".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    for (identity, permset) in [
        (owner, admin.id),
        (owner, narrow.id),
        (editor.id, narrow.id),
    ] {
        PermissionAssignmentRepository::create(
            &ctx.pool,
            CreatePermissionAssignmentInput { identity, permset },
        )
        .await?;
    }
    RuleRepository::update(
        &ctx.pool,
        rule.id,
        UpdateRuleInput {
            owner_identity: Some(Patch::Set(owner)),
            permission_set_refs: Some(Patch::Set(vec![])),
            ..Default::default()
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    use attune_common::repositories::{
        delegated_execution::DelegatedExecutionRepository,
        event::{CreateEnforcementInput, EnforcementRepository},
        executable_snapshot::ExecutableSnapshotRepository,
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let pack =
        attune_common::repositories::pack::PackRepository::find_by_ref(&ctx.pool, &rule.pack_ref)
            .await?
            .unwrap();
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &attune_common::repositories::component_lifecycle::PackProjectionIds {
            actions: vec![rule.action.unwrap(), privileged.id],
            triggers: vec![rule.trigger.unwrap()],
            ..Default::default()
        },
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action(&mut *tx, rule.action.unwrap()).await?;
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    let pending = EnforcementRepository::create_or_get_by_rule_event_pinned(
        &mut tx,
        CreateEnforcementInput {
            rule: Some(rule.id),
            rule_ref: rule_ref.clone(),
            trigger_ref: rule.trigger_ref.clone(),
            config: Some(json!({})),
            event: None,
            status: attune_common::models::EnforcementStatus::Created,
            payload: json!({}),
            condition: attune_common::models::EnforcementCondition::All,
            conditions: json!({}),
        },
        &snapshot,
    )
    .await?
    .enforcement;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Enforcement, pending.id, &binding)
        .await?;
    DelegatedExecutionRepository::accept_enforcement(&mut tx, &pending, owner, &[]).await?;
    tx.commit().await?;
    let tokens = [
        generate_token(
            editor.id,
            "different_editor",
            &jwt_config(),
            TokenType::Access,
        )?,
        helpers::delegated_source::token(&ctx, owner, &[narrow.r#ref.clone()]).await?,
    ];
    for token in &tokens {
        for change in [
            json!({"action_ref":privileged.r#ref}),
            json!({"action_params":{"credential":"{{ keystore['system.api_token'] }}"}}),
            json!({"trigger_params":{"credential":"{{ keystore['system.api_token'] }}"}}),
            json!({"trace_tag_template":"{{ keystore['system.api_token'] }}"}),
            json!({"permission_set_refs":[admin.r#ref]}),
        ] {
            ctx.put(&format!("/api/v1/rules/{rule_ref}"), change, Some(token))
                .await?
                .assert_status(StatusCode::FORBIDDEN);
        }
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"label":"Editorial change"}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK);
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"action_params":{"message":"covered"},"conditions":{}}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK);
    }
    // A retained named selection is checked for every behavioral edit, including
    // conditions and trace, even when the request does not mention the selection.
    RuleRepository::update(
        &ctx.pool,
        rule.id,
        UpdateRuleInput {
            permission_set_refs: Some(Patch::Set(vec![admin.r#ref.clone()])),
            ..Default::default()
        },
    )
    .await?;
    for token in &tokens {
        for change in [
            json!({"conditions":{}}),
            json!({"trace_tag_template":"changed"}),
            json!({"action_params":{}}),
        ] {
            ctx.put(&format!("/api/v1/rules/{rule_ref}"), change, Some(token))
                .await?
                .assert_status(StatusCode::FORBIDDEN);
        }
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"label":"Still editable"}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK);
    }
    // A mixed public/protected retained binding lets public selectors pass with
    // Read alone; secret selectors require Decrypt without restoring the value.
    let mut pack =
        attune_common::repositories::pack::PackRepository::find_by_ref(&ctx.pool, &rule.pack_ref)
            .await?
            .unwrap();
    pack.conf_schema =
        json!({"public_name":{"type":"string"},"private_name":{"type":"string","secret":true}});
    let mut tx = ctx.pool.begin().await?;
    attune_common::repositories::component_configuration::ComponentConfigurationRepository::bind_pack(
        &mut tx, &pack, Some(&json!({"public_name":"fixture","private_name":"test-only-config"})), ctx.state.config.security.encryption_key.as_deref(),
    ).await?;
    tx.commit().await?;
    let config_read = PermissionSetRepository::create(&ctx.pool,CreatePermissionSetInput {
        r#ref: "test.config_read".into(),pack: None,pack_ref: None,label: None,description: None,
        grants: json!([{"resource":"packs","actions":["read"],"constraints":{"refs":[rule.pack_ref]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: editor.id,
            permset: config_read.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let config_token = generate_token(
        editor.id,
        "different_editor",
        &jwt_config(),
        TokenType::Access,
    )?;
    for expression in [
        "{{ pack.config.public_name }}",
        "{{ pack['config']['public_name'] }}",
    ] {
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"action_ref":rule.action_ref,"permission_set_refs":[],
            "action_params":{"message":expression}}),
            Some(&config_token),
        )
        .await?
        .assert_status(StatusCode::OK);
    }
    for expression in ["{{ pack.config.private_name }}", "{{ pack.config }}"] {
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"action_params":{"message":expression}}),
            Some(&config_token),
        )
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    }
    // Standard on an unrelated source credential cannot widen to this rule's
    // action/pack, even when that credential can Execute the action itself.
    let standard_token =
        helpers::delegated_source::token(&ctx, owner, &[narrow.r#ref.clone(), "standard".into()])
            .await?;
    ctx.put(
        &format!("/api/v1/rules/{rule_ref}"),
        json!({"permission_set_refs":["standard"]}),
        Some(&standard_token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    let owner_token = generate_token(
        owner,
        "rule_authz_sponsor",
        &jwt_config(),
        TokenType::Access,
    )?;
    ctx.put(
        &format!("/api/v1/rules/{rule_ref}"),
        json!({"action_ref":privileged.r#ref,"permission_set_refs":[admin.r#ref],
        "action_params":{"credential":"{{ keystore['system.api_token'] }}"}}),
        Some(&owner_token),
    )
    .await?
    .assert_status(StatusCode::OK);
    for expression in [
        "{{ pack.config.public_name }}",
        "{{ pack['config']['public_name'] }}",
    ] {
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"action_params":{"message":expression}}),
            Some(&owner_token),
        )
        .await?
        .assert_status(StatusCode::OK);
    }
    for expression in [
        "{{ keystore[pack.config.key_ref] }}",
        "{{ keystore[event.payload.key_ref] }}",
    ] {
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"action_params":{"message":expression}}),
            Some(&owner_token),
        )
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    }
    // Labels stay editable even when the existing input needs stronger grants.
    ctx.put(
        &format!("/api/v1/rules/{rule_ref}"),
        json!({"label":"Public config retained"}),
        Some(&tokens[0]),
    )
    .await?
    .assert_status(StatusCode::OK);
    let stored = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
        .await?
        .unwrap();
    assert_eq!(stored.owner_identity, Some(owner));
    assert_eq!(stored.permission_set_refs, Some(vec![admin.r#ref]));
    let mut tx = ctx.pool.begin().await?;
    let retained = <EnforcementRepository as attune_common::repositories::FindById>::find_by_id(
        &mut *tx, pending.id,
    )
    .await?
    .unwrap();
    assert_eq!(retained.executable_snapshot, pending.executable_snapshot);
    assert_eq!(
        DelegatedExecutionRepository::enforcement_selection(&mut tx, &retained).await?,
        (owner, vec![])
    );
    tx.commit().await?;
    // Managed input coverage comes from the retained deployment capture, even
    // after the live permission-set definition no longer contains these grants.
    use attune_common::repositories::rule_authority::RuleAuthorityRepository;
    RuleRepository::update(
        &ctx.pool,
        rule.id,
        UpdateRuleInput {
            is_adhoc: Some(false),
            permission_set_refs: Some(Patch::Set(vec![])),
            ..Default::default()
        },
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    RuleAuthorityRepository::set_input_refs(&mut tx, rule.id, &[admin.r#ref.clone()]).await?;
    RuleAuthorityRepository::capture_deployment(&mut tx, rule.pack).await?;
    let managed = RuleRepository::find_by_ref(&mut *tx, &rule_ref)
        .await?
        .unwrap();
    let captured = RuleAuthorityRepository::current(&mut tx, &managed)
        .await?
        .unwrap();
    tx.commit().await?;
    PermissionSetRepository::update(
        &ctx.pool,
        admin.id,
        attune_common::repositories::identity::UpdatePermissionSetInput {
            grants: Some(json!([])),
            ..Default::default()
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    for token in &tokens {
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"label":"Managed label"}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK);
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"action_ref":rule.action_ref,
            "action_params":{},"conditions":{}}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    }
    let mut tx = ctx.pool.begin().await?;
    let after = RuleRepository::find_by_ref(&mut *tx, &rule_ref)
        .await?
        .unwrap();
    assert_eq!(after.owner_identity, managed.owner_identity);
    assert_eq!(
        serde_json::to_value(
            RuleAuthorityRepository::current(&mut tx, &after)
                .await?
                .unwrap()
        )?,
        serde_json::to_value(captured)?
    );
    tx.commit().await?;
    ctx.cleanup().await?;
    Ok(())
}
