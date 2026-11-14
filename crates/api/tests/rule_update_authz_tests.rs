mod helpers;

use attune_common::{
    auth::jwt::{generate_access_token, generate_worker_token, JwtConfig},
    repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
            UpdateIdentityInput,
        },
        rule::{CreateRuleInput, RuleRepository, UpdateRuleInput},
        Create, Delete, FindByRef, Update,
    },
};
use axum::http::StatusCode;
use helpers::{create_test_action, create_test_pack, create_test_trigger, Result, TestContext};
use serde_json::{json, Value};
use std::time::Duration;
use tower::ServiceExt;

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
        helpers::delegated_source::token(&ctx, owner, std::slice::from_ref(&narrow.r#ref)).await?,
    ];
    for (index, token) in tokens.iter().enumerate() {
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
            json!({"action_params":{"message":format!("covered-{index}")},"conditions":{}}),
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
            json!({"conditions":{"==":[1,1]}}),
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
    // Inherited action defaults are also effective permission selections.
    use attune_common::repositories::action::{ActionRepository, UpdateActionInput};
    ActionRepository::update(
        &ctx.pool,
        rule.action.unwrap(),
        UpdateActionInput {
            default_execution_permission_set_refs: Some(vec![admin.r#ref.clone()]),
            ..Default::default()
        },
    )
    .await?;
    RuleRepository::update(
        &ctx.pool,
        rule.id,
        UpdateRuleInput {
            permission_set_refs: Some(Patch::Clear),
            ..Default::default()
        },
    )
    .await?;
    for token in &tokens {
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            json!({"trace_tag_template":"inherited-default-edit"}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    }
    ActionRepository::update(
        &ctx.pool,
        rule.action.unwrap(),
        UpdateActionInput {
            default_execution_permission_set_refs: Some(vec![]),
            ..Default::default()
        },
    )
    .await?;
    RuleRepository::update(
        &ctx.pool,
        rule.id,
        UpdateRuleInput {
            permission_set_refs: Some(Patch::Set(vec![admin.r#ref.clone()])),
            ..Default::default()
        },
    )
    .await?;
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
    assert_eq!(stored.permission_set_refs, Some(vec![admin.r#ref.clone()]));
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
    assert_eq!(
        serde_json::to_value(
            SchemaContractRepository::find(&mut tx, BindingOwner::Enforcement, pending.id)
                .await?
                .unwrap()
        )?,
        serde_json::to_value(&binding)?,
        "pending enforcement retains its original schema binding"
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
    RuleAuthorityRepository::set_input_refs(&mut tx, rule.id, std::slice::from_ref(&admin.r#ref))
        .await?;
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

fn full_form_update(rule: &attune_common::models::Rule, label: &str) -> serde_json::Value {
    json!({"label":label,"description":rule.description,"action_ref":rule.action_ref,
        "trigger_ref":rule.trigger_ref,"conditions":rule.conditions,"action_params":rule.action_params,
        "trigger_params":rule.trigger_params,"permission_set_refs":rule.permission_set_refs,
        "trace_tag_template":rule.trace_tag_template,"enabled":rule.enabled,
        "sensor_worker_selector":rule.sensor_worker_selector,
        "sensor_worker_tolerations":rule.sensor_worker_tolerations,
        "sensor_worker_affinity":rule.sensor_worker_affinity})
}

#[tokio::test]
async fn full_form_metadata_retains_update_semantics_but_placement_spends_editor_authority(
) -> Result<()> {
    use attune_common::{
        auth::jwt::{generate_token, TokenType},
        repositories::{rule::UpdateRuleInput, Patch, Update},
    };
    let ctx = TestContext::new().await?;
    let (rule_ref, sponsor) = seed_rule_and_identity(&ctx, "full_form").await?;
    let editor = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "full_form_editor".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    let metadata = PermissionSetRepository::create(&ctx.pool,CreatePermissionSetInput {
        r#ref:"test.full_form_metadata".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"rules","actions":["update"],"constraints":{"refs":[rule_ref]}},
            {"resource":"actions","actions":["read"]},{"resource":"triggers","actions":["read"]}]),
    }).await?;
    let admin = PermissionSetRepository::create(&ctx.pool,CreatePermissionSetInput {
        r#ref:"test.full_form_sponsor".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"rules","actions":["update"]},{"resource":"actions","actions":["execute"]}]),
    }).await?;
    for (identity, permset) in [
        (sponsor, metadata.id),
        (editor.id, metadata.id),
        (sponsor, admin.id),
    ] {
        PermissionAssignmentRepository::create(
            &ctx.pool,
            CreatePermissionAssignmentInput { identity, permset },
        )
        .await?;
    }
    let rule = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
        .await?
        .unwrap();
    RuleRepository::update(
        &ctx.pool,
        rule.id,
        UpdateRuleInput {
            owner_identity: Some(Patch::Set(sponsor)),
            permission_set_refs: Some(Patch::Set(vec![])),
            ..Default::default()
        },
    )
    .await?;
    // Placement is valid only for triggers backed by a managed sensor. Keep
    // this test sensor disabled: positive writes prove structural authoring
    // admission and persistence, without claiming live-worker dispatch coverage.
    use attune_common::repositories::{
        runtime::{CreateRuntimeInput, RuntimeRepository},
        trigger::{CreateSensorInput, SensorRepository, TriggerRepository, UpdateTriggerInput},
    };
    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: format!("{}.native", rule.pack_ref),
            pack: Some(rule.pack),
            pack_ref: Some(rule.pack_ref.clone()),
            description: None,
            name: "native".into(),
            aliases: vec![],
            distributions: json!({}),
            installation: None,
            execution_config: json!({}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await?;
    let sensor = SensorRepository::create(
        &ctx.pool,
        CreateSensorInput {
            r#ref: format!("{}.source", rule.pack_ref),
            pack: Some(rule.pack),
            pack_ref: Some(rule.pack_ref.clone()),
            label: "Placement fixture".into(),
            description: None,
            entrypoint: "source".into(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref,
            runtime_version_constraint: None,
            enabled: false,
            param_schema: None,
            config: Some(json!({})),
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
        },
    )
    .await?;
    TriggerRepository::update(
        &ctx.pool,
        rule.trigger.unwrap(),
        UpdateTriggerInput {
            sensor: Some(Patch::Set(sensor.id)),
            sensor_ref: Some(Patch::Set(sensor.r#ref)),
            ..Default::default()
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let tokens = [
        generate_token(
            editor.id,
            "full_form_editor",
            &jwt_config(),
            TokenType::Access,
        )?,
        helpers::delegated_source::token(&ctx, sponsor, std::slice::from_ref(&metadata.r#ref))
            .await?,
    ];
    let placement_edits = [
        json!({"sensor_worker_selector":{"zone":"other"}}),
        json!({"sensor_worker_tolerations":[{"key":"dedicated","operator":"exists"}]}),
        json!({"sensor_worker_affinity":{"required":[{"match_labels":{"zone":"other"}}]}}),
    ];
    for token in &tokens {
        let before = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
            .await?
            .unwrap();
        let payload = full_form_update(&before, "Full form label");
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            payload.clone(),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK);
        let mut behavioral = payload;
        behavioral["label"] = json!("Must roll back with behavior");
        behavioral["trace_tag_template"] = json!("real behavior change");
        ctx.put(
            &format!("/api/v1/rules/{rule_ref}"),
            behavioral,
            Some(token),
        )
        .await?
        .assert_status(StatusCode::FORBIDDEN);
        let before = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
            .await?
            .unwrap();
        for change in &placement_edits {
            let denied = ctx
                .put(
                    &format!("/api/v1/rules/{rule_ref}"),
                    change.clone(),
                    Some(token),
                )
                .await?;
            let status = denied.status();
            let body: serde_json::Value = denied.json().await?;
            assert_eq!(status, StatusCode::FORBIDDEN, "{}", body["error"]);
            assert_eq!(body["error"], "Insufficient permissions: actions:execute");
            assert_eq!(
                serde_json::to_value(
                    RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
                        .await?
                        .unwrap()
                )?,
                serde_json::to_value(&before)?
            );
        }
        assert_eq!(before.label, "Full form label");
        assert_eq!(before.owner_identity, Some(sponsor));
    }
    let covered = generate_token(
        sponsor,
        "rule_authz_full_form",
        &jwt_config(),
        TokenType::Access,
    )?;
    for change in placement_edits {
        let response = ctx
            .put(&format!("/api/v1/rules/{rule_ref}"), change, Some(&covered))
            .await?;
        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "covered placement: {}",
            body["error"]
        );
    }
    let placed = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
        .await?
        .unwrap();
    assert_eq!(placed.sensor_worker_selector, json!({"zone":"other"}));
    assert_eq!(
        placed.sensor_worker_tolerations,
        json!([{"key":"dedicated","operator":"exists"}])
    );
    assert_eq!(
        placed.sensor_worker_affinity,
        json!({"required":[{"match_labels":{"zone":"other"}}]})
    );
    assert_eq!(placed.owner_identity, Some(sponsor));
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn config_sources_match_actual_pack_ownership_and_access_role_membership() -> Result<()> {
    use attune_common::{
        auth::jwt::{generate_token, TokenType},
        repositories::{
            identity::{CreateIdentityRoleAssignmentInput, IdentityRoleAssignmentRepository},
            pack::PackRepository,
            rule::UpdateRuleInput,
            Delete, Patch, Update,
        },
    };
    let ctx = TestContext::new().await?;
    let sponsor = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "config_source_sponsor".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    for (suffix, scope, ownership, role_member, allowed) in [
        ("unowned", json!({"owner":"none"}), "none", false, true),
        (
            "owned_none",
            json!({"owner":"none"}),
            "sponsor",
            false,
            false,
        ),
        ("owned_self", json!({"owner":"self"}), "editor", false, true),
        (
            "foreign_self",
            json!({"owner":"self"}),
            "sponsor",
            false,
            false,
        ),
        ("role_self", json!({"owner":"self"}), "sponsor", true, true),
        (
            "role_pack",
            json!({"pack_owner":"self"}),
            "sponsor",
            true,
            true,
        ),
    ] {
        let (rule_ref, editor) = seed_rule_and_identity(&ctx, suffix).await?;
        let rule = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
            .await?
            .unwrap();
        if ownership != "none" {
            PackRepository::set_installed_by(
                &ctx.pool,
                rule.pack,
                if ownership == "editor" {
                    editor
                } else {
                    sponsor.id
                },
            )
            .await?;
        }
        RuleRepository::update(
            &ctx.pool,
            rule.id,
            UpdateRuleInput {
                owner_identity: Some(Patch::Set(sponsor.id)),
                permission_set_refs: Some(Patch::Set(vec![])),
                ..Default::default()
            },
        )
        .await?;
        let mut pack = PackRepository::find_by_ref(&ctx.pool, &rule.pack_ref)
            .await?
            .unwrap();
        pack.conf_schema = json!({"private_name":{"type":"string","secret":true}});
        let mut tx = ctx.pool.begin().await?;
        attune_common::repositories::component_configuration::ComponentConfigurationRepository::bind_pack(
            &mut tx,&pack,Some(&json!({"private_name":"fixture-only-config"})),ctx.state.config.security.encryption_key.as_deref(),
        ).await?;
        PackRepository::set_owner_roles(&mut tx, pack.id, vec!["rule-config-maintainers".into()])
            .await?;
        tx.commit().await?;
        let member = if role_member {
            Some(
                IdentityRoleAssignmentRepository::create(
                    &ctx.pool,
                    CreateIdentityRoleAssignmentInput {
                        identity: editor,
                        role: "rule-config-maintainers".into(),
                        source: "test".into(),
                        managed: false,
                    },
                )
                .await?,
            )
        } else {
            None
        };
        let grants = json!([{"resource":"rules","actions":["update"],"constraints":{"refs":[rule_ref]}},
            {"resource":"actions","actions":["execute"],"constraints":{"refs":[rule.action_ref]}},
            {"resource":"packs","actions":["read","decrypt"],"constraints":scope}]);
        let permission = PermissionSetRepository::create(
            &ctx.pool,
            CreatePermissionSetInput {
                r#ref: format!("test.config_{suffix}"),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants: grants.clone(),
            },
        )
        .await?;
        PermissionAssignmentRepository::create(
            &ctx.pool,
            CreatePermissionAssignmentInput {
                identity: editor,
                permset: permission.id,
            },
        )
        .await?;
        attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
        let token = generate_token(
            editor,
            &format!("rule_authz_{suffix}"),
            &jwt_config(),
            TokenType::Access,
        )?;
        let response = ctx
            .put(
                &format!("/api/v1/rules/{rule_ref}"),
                json!({"action_params":{"message":"{{ pack.config.private_name }}","attempt":1}}),
                Some(&token),
            )
            .await?;
        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        assert_eq!(
            status,
            if allowed {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "config case {suffix}: {}",
            body["error"]
        );
        if !allowed {
            assert_eq!(body["error"], "Insufficient permissions: packs:read");
        }
        if suffix == "unowned" || suffix == "owned_none" {
            let execution = helpers::delegated_source::token(
                &ctx,
                editor,
                std::slice::from_ref(&permission.r#ref),
            )
            .await?;
            ctx.put(
                &format!("/api/v1/rules/{rule_ref}"),
                json!({"action_params":{"message":"{{ pack.config.private_name }}","attempt":2}}),
                Some(&execution),
            )
            .await?
            .assert_status(if allowed {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            });
        }
        if let Some(member) = member {
            let mut token_grants = grants;
            token_grants[2]["constraints"] = json!({"owner":"none"});
            let narrow = PermissionSetRepository::create(
                &ctx.pool,
                CreatePermissionSetInput {
                    r#ref: format!("test.config_narrow_{suffix}"),
                    pack: None,
                    pack_ref: None,
                    label: None,
                    description: None,
                    grants: token_grants,
                },
            )
            .await?;
            PermissionAssignmentRepository::create(
                &ctx.pool,
                CreatePermissionAssignmentInput {
                    identity: editor,
                    permset: narrow.id,
                },
            )
            .await?;
            attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
            let execution =
                helpers::delegated_source::token(&ctx, editor, std::slice::from_ref(&narrow.r#ref))
                    .await?;
            ctx.put(
                &format!("/api/v1/rules/{rule_ref}"),
                json!({"action_params":{"message":"{{ pack.config.private_name }}","attempt":2}}),
                Some(&execution),
            )
            .await?
            .assert_status(StatusCode::FORBIDDEN);
            IdentityRoleAssignmentRepository::delete(&ctx.pool, member.id).await?;
            attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
            ctx.put(
                &format!("/api/v1/rules/{rule_ref}"),
                json!({"action_params":{"message":"{{ pack.config.private_name }}","attempt":3}}),
                Some(&token),
            )
            .await?
            .assert_status(StatusCode::FORBIDDEN);
            let before = RuleRepository::find_by_ref(&ctx.pool, &rule_ref)
                .await?
                .unwrap();
            ctx.put(
                &format!("/api/v1/rules/{rule_ref}"),
                full_form_update(&before, "Metadata after role revocation"),
                Some(&token),
            )
            .await?
            .assert_status(StatusCode::OK);
        }
    }
    ctx.cleanup().await?;
    Ok(())
}

async fn assign_rule_update(
    ctx: &TestContext,
    identity: i64,
    reference: &str,
    grants: Value,
) -> Result<i64> {
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: reference.into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    Ok(PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?
    .id)
}

#[tokio::test]
async fn delegated_and_worker_put_enabled_transitions_deny_before_any_rule_effect() -> Result<()> {
    let ctx = TestContext::new().await?;
    let (reference, identity) = seed_rule_and_identity(&ctx, "enabled_denial").await?;
    let permission = "test.enabled_denial";
    assign_rule_update(
        &ctx,
        identity,
        permission,
        json!([
            {"resource":"rules","actions":["update"],"constraints":{"refs":[reference]}}
        ]),
    )
    .await?;
    let delegated = helpers::delegated_source::token(&ctx, identity, &[permission.into()]).await?;
    let worker =
        generate_worker_token(identity, "enabled-worker", &ctx.state.jwt_config, Some(600))?;
    for initial in [true, false] {
        let rule = RuleRepository::find_by_ref(&ctx.pool, &reference)
            .await?
            .unwrap();
        let before = RuleRepository::update(
            &ctx.pool,
            rule.id,
            UpdateRuleInput {
                enabled: Some(initial),
                ..Default::default()
            },
        )
        .await?;
        for bearer in [&delegated, &worker] {
            for payload in [
                json!({"enabled":!initial}),
                json!({"enabled":!initial,"label":"Must not change","conditions":{"changed":true},"action_params":{"changed":true}}),
            ] {
                let response = ctx
                    .put(&format!("/api/v1/rules/{reference}"), payload, Some(bearer))
                    .await?;
                assert_eq!(
                    response.status(),
                    StatusCode::FORBIDDEN,
                    "{}",
                    response.text().await?
                );
                let after = RuleRepository::find_by_ref(&ctx.pool, &reference)
                    .await?
                    .unwrap();
                assert_eq!(after.enabled, before.enabled);
                assert_eq!(after.updated, before.updated);
                assert_eq!(after.label, before.label);
                assert_eq!(after.conditions, before.conditions);
                assert_eq!(after.action_params, before.action_params);
                assert_eq!(after.permission_set_refs, before.permission_set_refs);
            }
        }
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn delegated_metadata_and_unchanged_full_form_enabled_keep_rule_update_caps() -> Result<()> {
    let ctx = TestContext::new().await?;
    let (reference, identity) = seed_rule_and_identity(&ctx, "enabled_noop").await?;
    let (adjacent, _) = seed_rule_and_identity(&ctx, "enabled_adjacent").await?;
    let permission = "test.enabled_noop";
    // No Actions/Execute grant; the token only carries the existing scoped PUT authority.
    assign_rule_update(
        &ctx,
        identity,
        permission,
        json!([
            {"resource":"rules","actions":["update"],"constraints":{"refs":[reference]}}
        ]),
    )
    .await?;
    let bearer = helpers::delegated_source::token(&ctx, identity, &[permission.into()]).await?;
    ctx.put(
        &format!("/api/v1/rules/{reference}"),
        json!({"label":"Metadata only"}),
        Some(&bearer),
    )
    .await?
    .assert_status(StatusCode::OK);
    for initial in [false, true] {
        let rule = RuleRepository::find_by_ref(&ctx.pool, &reference)
            .await?
            .unwrap();
        let before = RuleRepository::update(
            &ctx.pool,
            rule.id,
            UpdateRuleInput {
                enabled: Some(initial),
                ..Default::default()
            },
        )
        .await?;
        let payload = json!({
            "enabled":before.enabled,"label":before.label,"description":before.description,
            "action_ref":before.action_ref,"trigger_ref":before.trigger_ref,
            "conditions":before.conditions,"action_params":before.action_params,
            "trigger_params":before.trigger_params,"permission_set_refs":before.permission_set_refs,
            "trace_tag_template":before.trace_tag_template,
            "sensor_worker_selector":before.sensor_worker_selector,
            "sensor_worker_tolerations":before.sensor_worker_tolerations,
            "sensor_worker_affinity":before.sensor_worker_affinity
        });
        let response = ctx
            .put(
                &format!("/api/v1/rules/{reference}"),
                payload,
                Some(&bearer),
            )
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await?
        );
        let after = RuleRepository::find_by_ref(&ctx.pool, &reference)
            .await?
            .unwrap();
        assert_eq!(after.enabled, initial);
        assert_eq!(after.action, before.action);
        assert_eq!(after.owner_identity, before.owner_identity);
    }
    let before = RuleRepository::find_by_ref(&ctx.pool, &adjacent)
        .await?
        .unwrap();
    ctx.put(
        &format!("/api/v1/rules/{adjacent}"),
        json!({"enabled":before.enabled,"label":"Outside cap"}),
        Some(&bearer),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    let after = RuleRepository::find_by_ref(&ctx.pool, &adjacent)
        .await?
        .unwrap();
    assert_eq!(after.updated, before.updated);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn access_enabled_transitions_require_target_scope_but_not_action_execute() -> Result<()> {
    let ctx = TestContext::new().await?;
    let (reference, identity) = seed_rule_and_identity(&ctx, "enabled_access").await?;
    let (adjacent, _) = seed_rule_and_identity(&ctx, "enabled_other").await?;
    assign_rule_update(
        &ctx,
        identity,
        "test.enabled_access",
        json!([
            {"resource":"rules","actions":["update"],"constraints":{"refs":[reference]}}
        ]),
    )
    .await?;
    let bearer = generate_access_token(identity, "enabled-access", &ctx.state.jwt_config)?;
    for enabled in [false, true] {
        let response = ctx
            .put(
                &format!("/api/v1/rules/{reference}"),
                json!({"enabled":enabled}),
                Some(&bearer),
            )
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await?
        );
        assert_eq!(
            RuleRepository::find_by_ref(&ctx.pool, &reference)
                .await?
                .unwrap()
                .enabled,
            enabled
        );
    }
    let before = RuleRepository::find_by_ref(&ctx.pool, &adjacent)
        .await?
        .unwrap();
    ctx.put(
        &format!("/api/v1/rules/{adjacent}"),
        json!({"enabled":!before.enabled}),
        Some(&bearer),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        RuleRepository::find_by_ref(&ctx.pool, &adjacent)
            .await?
            .unwrap()
            .updated,
        before.updated
    );
    ctx.cleanup().await?;
    Ok(())
}

async fn wait_for_rule_request(ctx: &TestContext, blocker: i32) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE $1 = ANY(pg_blocking_pids(a.pid)))")
                .bind(blocker).fetch_one(&ctx.pool).await?;
            if waiting { return Ok::<_, sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await.map_err(|_| format!("Rule PUT did not wait on owned backend {blocker}"))??;
    Ok(())
}

async fn enabled_put_after_revocation(freeze: bool) -> Result<()> {
    use attune_common::repositories::schema_type::SchemaTypeRepository;
    use axum::{body::Body, http::Request};
    let ctx = TestContext::new_without_authorization_metadata_cache().await?;
    let (reference, identity) = seed_rule_and_identity(&ctx, "enabled_revoke").await?;
    let assignment = assign_rule_update(
        &ctx,
        identity,
        "test.enabled_revoke",
        json!([
            {"resource":"rules","actions":["update"],"constraints":{"refs":[reference]}}
        ]),
    )
    .await?;
    let bearer = generate_access_token(identity, "enabled-revoke", &ctx.state.jwt_config)?;
    let before = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    let mut blocker = ctx.pool.begin().await?;
    SchemaTypeRepository::lock_reclamation(&mut blocker).await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let app = ctx.app.clone();
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/rules/{reference}"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::from(r#"{"enabled":false}"#))?;
    let mut requests = tokio::task::JoinSet::new();
    requests.spawn(async move { app.oneshot(request).await.unwrap().status() });
    let observation: Result<()> = async {
        // Generic IAM has completed before this real graph wait. The later
        // transition guard must re-read the now-revoked authority in its transaction.
        wait_for_rule_request(&ctx, pid).await?;
        if freeze {
            IdentityRepository::update(
                &ctx.pool,
                identity,
                UpdateIdentityInput {
                    frozen: Some(true),
                    ..Default::default()
                },
            )
            .await?;
        } else {
            PermissionAssignmentRepository::delete(&ctx.pool, assignment).await?;
        }
        Ok(())
    }
    .await;
    let rollback = blocker.rollback().await;
    if observation.is_err() || rollback.is_err() {
        requests.abort_all();
    }
    let joined = tokio::time::timeout(Duration::from_secs(20), requests.join_next()).await;
    if joined.is_err() {
        requests.abort_all();
        while requests.join_next().await.is_some() {}
    }
    let after = RuleRepository::find_by_ref(&ctx.pool, &reference).await;
    ctx.cleanup().await?;
    observation?;
    rollback?;
    let status = joined?.ok_or("Rule PUT task disappeared")??;
    assert_eq!(
        status,
        if freeze {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::FORBIDDEN
        }
    );
    let after = after?.unwrap();
    assert_eq!(after.enabled, before.enabled);
    assert_eq!(after.updated, before.updated);
    Ok(())
}

#[tokio::test]
async fn put_enabled_rechecks_revoked_assignment_after_graph_wait() -> Result<()> {
    enabled_put_after_revocation(false).await
}

#[tokio::test]
async fn put_enabled_rechecks_frozen_identity_after_graph_wait() -> Result<()> {
    enabled_put_after_revocation(true).await
}

#[tokio::test]
async fn put_enabled_holds_fresh_grant_until_the_rule_write_commits() -> Result<()> {
    use axum::{body::Body, http::Request};
    let ctx = TestContext::new_without_authorization_metadata_cache().await?;
    let (reference, identity) = seed_rule_and_identity(&ctx, "enabled_held").await?;
    let assignment = assign_rule_update(
        &ctx,
        identity,
        "test.enabled_held",
        json!([
            {"resource":"rules","actions":["update"],"constraints":{"refs":[reference]}}
        ]),
    )
    .await?;
    let bearer = generate_access_token(identity, "enabled-held", &ctx.state.jwt_config)?;
    // SHARE admits the locked current-row read, but fences the later UPDATE.
    let mut blocker = ctx.pool.begin().await?;
    sqlx::query("LOCK TABLE rule IN SHARE MODE")
        .execute(&mut *blocker)
        .await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let app = ctx.app.clone();
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/rules/{reference}"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::from(r#"{"enabled":false}"#))?;
    let mut requests = tokio::task::JoinSet::new();
    requests.spawn(
        async move { Ok::<_, String>((true, app.oneshot(request).await.unwrap().status())) },
    );
    let observation: Result<()> = async {
        wait_for_rule_request(&ctx, pid).await?;
        let mut revoker = ctx.pool.begin().await?;
        let revoker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *revoker)
            .await?;
        requests.spawn(async move {
            PermissionAssignmentRepository::delete(&mut *revoker, assignment)
                .await
                .map_err(|error| error.to_string())?;
            revoker.commit().await.map_err(|error| error.to_string())?;
            Ok((false, StatusCode::OK))
        });
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let blocked: bool =
                    sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1)) > 0")
                        .bind(revoker_pid)
                        .fetch_one(&ctx.pool)
                        .await?;
                if blocked {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "Grant revocation did not wait for the authorized rule write")??;
        Ok(())
    }
    .await;
    let rollback = blocker.rollback().await;
    if observation.is_err() || rollback.is_err() {
        requests.abort_all();
    }
    let joined = tokio::time::timeout(Duration::from_secs(20), async {
        let mut completed = Vec::new();
        while let Some(result) = requests.join_next().await {
            completed.push(result);
        }
        completed
    })
    .await;
    if joined.is_err() {
        requests.abort_all();
        while requests.join_next().await.is_some() {}
    }
    let after = RuleRepository::find_by_ref(&ctx.pool, &reference).await;
    ctx.cleanup().await?;
    observation?;
    rollback?;
    let completed = joined?;
    assert_eq!(completed.len(), 2);
    for result in completed {
        let (_, status) =
            result?.map_err(|error| format!("Rule publication/revocation failed: {error}"))?;
        assert_eq!(status, StatusCode::OK);
    }
    assert!(!after?.unwrap().enabled);
    Ok(())
}

#[tokio::test]
async fn access_put_enable_keeps_live_sensor_placement_admission_and_rolls_back() -> Result<()> {
    use attune_common::repositories::{
        runtime::{CreateRuntimeInput, RuntimeRepository},
        trigger::{CreateSensorInput, SensorRepository, TriggerRepository, UpdateTriggerInput},
        Patch,
    };
    let ctx = TestContext::new().await?;
    let (reference, identity) = seed_rule_and_identity(&ctx, "enabled_placement").await?;
    let rule = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    RuleRepository::update(
        &ctx.pool,
        rule.id,
        UpdateRuleInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: format!("{}.native", rule.pack_ref),
            pack: Some(rule.pack),
            pack_ref: Some(rule.pack_ref.clone()),
            description: None,
            name: "native".into(),
            aliases: vec![],
            distributions: json!({}),
            installation: None,
            execution_config: json!({}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await?;
    // Model an enabled managed sensor whose worker is unavailable. No service
    // process is started; live placement must reject activating this rule.
    let sensor = SensorRepository::create(
        &ctx.pool,
        CreateSensorInput {
            r#ref: format!("{}.sensor", rule.pack_ref),
            pack: Some(rule.pack),
            pack_ref: Some(rule.pack_ref.clone()),
            label: "Offline sensor".into(),
            description: None,
            entrypoint: "sensor.sh".into(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref,
            runtime_version_constraint: None,
            enabled: true,
            param_schema: None,
            config: None,
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
        },
    )
    .await?;
    TriggerRepository::update(
        &ctx.pool,
        rule.trigger.unwrap(),
        UpdateTriggerInput {
            sensor: Some(Patch::Set(sensor.id)),
            ..Default::default()
        },
    )
    .await?;
    assign_rule_update(
        &ctx,
        identity,
        "test.enabled_placement",
        json!([
            {"resource":"rules","actions":["update"],"constraints":{"refs":[reference]}}
        ]),
    )
    .await?;
    let bearer = generate_access_token(identity, "enabled-placement", &ctx.state.jwt_config)?;
    let before = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    let response = ctx
        .put(
            &format!("/api/v1/rules/{reference}"),
            json!({"enabled":true}),
            Some(&bearer),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        response.text().await?
    );
    let after = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    assert!(!after.enabled);
    assert_eq!(after.updated, before.updated);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn put_enabled_checks_actual_associated_pack_instead_of_a_stale_pack_ref() -> Result<()> {
    let ctx = TestContext::new().await?;
    let (reference, identity) = seed_rule_and_identity(&ctx, "enabled_structural").await?;
    // Deliberately model a historical/corrupt display association. The real
    // pack ID remains unchanged; no request field may supply lifecycle scope.
    sqlx::query("UPDATE rule SET pack_ref = 'unrelated_display_pack' WHERE ref = $1")
        .bind(&reference)
        .execute(&ctx.pool)
        .await?;
    assign_rule_update(
        &ctx,
        identity,
        "test.enabled_structural",
        json!([
            {"resource":"rules","actions":["update"],"constraints":{"pack_refs":["unrelated_display_pack"]}}
        ]),
    )
    .await?;
    let bearer = generate_access_token(identity, "enabled-structural", &ctx.state.jwt_config)?;
    let before = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    let response = ctx
        .put(
            &format!("/api/v1/rules/{reference}"),
            json!({"enabled":false}),
            Some(&bearer),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "{}",
        response.text().await?
    );
    let after = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    assert_eq!(after.enabled, before.enabled);
    assert_eq!(after.updated, before.updated);
    ctx.cleanup().await?;
    Ok(())
}

async fn assert_combined_update_denied_without_rule_effects(
    ctx: &TestContext,
    reference: &str,
    bearer: &str,
    payload: Value,
    expected_error: &str,
) -> Result<()> {
    let before = RuleRepository::find_by_ref(&ctx.pool, reference)
        .await?
        .unwrap();
    let response = ctx
        .put(&format!("/api/v1/rules/{reference}"), payload, Some(bearer))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{}", body["error"]);
    assert_eq!(body["code"], "FORBIDDEN");
    assert_eq!(body["error"], expected_error);
    assert_eq!(
        serde_json::to_value(
            RuleRepository::find_by_ref(&ctx.pool, reference)
                .await?
                .unwrap()
        )?,
        serde_json::to_value(before)?,
        "failed combined payload must leave every rule field unchanged"
    );
    Ok(())
}

#[tokio::test]
async fn mixed_enabled_and_behavior_updates_require_both_fresh_access_and_behavior_authority(
) -> Result<()> {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        Patch,
    };
    let ctx = TestContext::new().await?;
    let (reference, editor) = seed_rule_and_identity(&ctx, "combined_enabled_behavior").await?;
    let sponsor = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "combined_original_sponsor".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    let rule = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    RuleRepository::update(
        &ctx.pool,
        rule.id,
        UpdateRuleInput {
            owner_identity: Some(Patch::Set(sponsor.id)),
            permission_set_refs: Some(Patch::Set(vec![])),
            ..Default::default()
        },
    )
    .await?;
    ActionRepository::update(
        &ctx.pool,
        rule.action.unwrap(),
        UpdateActionInput {
            param_schema: Some(json!({"credential":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await?;
    seed_input_key(&ctx).await?;
    let update_ref = "test.combined_rule_update";
    let execute_ref = "test.combined_rule_execute";
    let read_ref = "test.combined_rule_key_read";
    let decrypt_ref = "test.combined_rule_key_decrypt";
    assign_rule_update(
        &ctx,
        editor,
        update_ref,
        json!([{"resource":"rules","actions":["update"],"constraints":{"refs":[reference]}}]),
    )
    .await?;
    let access = generate_access_token(editor, "combined-editor", &ctx.state.jwt_config)?;
    let selector = json!({"credential":"{{ keystore['system.api_token'] }}"});
    let mixed = json!({"enabled":false,"label":"Atomic mixed update","action_params":selector});
    assert_combined_update_denied_without_rule_effects(
        &ctx,
        &reference,
        &access,
        mixed.clone(),
        "Insufficient permissions: actions:execute",
    )
    .await?;
    assign_rule_update(&ctx, editor, execute_ref,
        json!([{"resource":"actions","actions":["execute"],"constraints":{"refs":[rule.action_ref]}}])
    ).await?;
    assert_combined_update_denied_without_rule_effects(
        &ctx,
        &reference,
        &access,
        mixed.clone(),
        "Rule input sources exceed editor authority",
    )
    .await?;
    assign_rule_update(&ctx, editor, read_ref,
        json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.api_token"],"owner_types":["system"]}}])
    ).await?;
    assert_combined_update_denied_without_rule_effects(
        &ctx,
        &reference,
        &access,
        mixed.clone(),
        "Rule input sources exceed editor authority",
    )
    .await?;
    assign_rule_update(&ctx, editor, decrypt_ref,
        json!([{"resource":"keys","actions":["decrypt"],"constraints":{"refs":["system.api_token"],"owner_types":["system"]}}])
    ).await?;
    let covered = ctx
        .put(&format!("/api/v1/rules/{reference}"), mixed, Some(&access))
        .await?;
    let status = covered.status();
    let body: Value = covered.json().await?;
    assert_eq!(status, StatusCode::OK, "{}", body["error"]);
    let disabled = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    assert!(!disabled.enabled);
    assert_eq!(disabled.action_params, selector);
    assert_eq!(disabled.owner_identity, Some(sponsor.id));

    let delegated = helpers::delegated_source::token(
        &ctx,
        editor,
        &[
            update_ref.into(),
            execute_ref.into(),
            read_ref.into(),
            decrypt_ref.into(),
        ],
    )
    .await?;
    // This exact delegated credential can change behavior when enabled is not
    // transitioning. Human IAM held by its subject never permits it to toggle.
    ctx.put(
        &format!("/api/v1/rules/{reference}"),
        json!({"trace_tag_template":"delegated behavior"}),
        Some(&delegated),
    )
    .await?
    .assert_status(StatusCode::OK);
    assert_combined_update_denied_without_rule_effects(
        &ctx,
        &reference,
        &delegated,
        json!({"enabled":true,"trace_tag_template":"Must not persist"}),
        "This ownership mutation requires an access token",
    )
    .await?;
    let metadata_only =
        helpers::delegated_source::token(&ctx, editor, &[update_ref.into()]).await?;
    let before = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    ctx.put(
        &format!("/api/v1/rules/{reference}"),
        full_form_update(&before, "Delegated unchanged enabled"),
        Some(&metadata_only),
    )
    .await?
    .assert_status(StatusCode::OK);
    let metadata = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    assert!(!metadata.enabled);
    assert_eq!(metadata.action_params, selector);
    assert_eq!(
        metadata.trace_tag_template.as_deref(),
        Some("delegated behavior")
    );
    assert_eq!(metadata.owner_identity, Some(sponsor.id));

    let enabled = ctx
        .put(
            &format!("/api/v1/rules/{reference}"),
            json!({"enabled":true,"trace_tag_template":"covered enabled behavior"}),
            Some(&access),
        )
        .await?;
    let status = enabled.status();
    let body: Value = enabled.json().await?;
    assert_eq!(status, StatusCode::OK, "{}", body["error"]);
    let final_rule = RuleRepository::find_by_ref(&ctx.pool, &reference)
        .await?
        .unwrap();
    assert!(final_rule.enabled);
    assert_eq!(
        final_rule.trace_tag_template.as_deref(),
        Some("covered enabled behavior")
    );
    assert_eq!(final_rule.action_params, selector);
    assert_eq!(final_rule.permission_set_refs, Some(vec![]));
    assert_eq!(final_rule.owner_identity, Some(sponsor.id));
    ctx.cleanup().await?;
    Ok(())
}
