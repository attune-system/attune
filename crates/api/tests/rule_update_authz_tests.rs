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
