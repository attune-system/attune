mod helpers;

use attune_common::repositories::{
    action::ActionRepository,
    identity::{
        CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
        IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
    },
    rule::RuleRepository,
    trigger::TriggerRepository,
    work_queue::WorkQueueRepository,
    Create, FindByRef,
};
use axum::http::StatusCode;
use helpers::{create_test_action, create_test_pack, create_test_trigger, Result, TestContext};
use serde_json::{json, Value};

async fn assign(ctx: &TestContext, identity: i64, name: &str, grants: Value) -> Result<()> {
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: name.into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn delegated_mutations_require_target_grants_even_with_admin_sponsor() -> Result<()> {
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "mutation_authz").await?;
    let action =
        create_test_action(&ctx.pool, pack.id, &pack.r#ref, "mutation_authz.target").await?;
    let trigger = create_test_trigger(&ctx.pool, pack.id, "mutation_authz.trigger").await?;
    let identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "mutation_admin".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    assign(
        &ctx,
        identity.id,
        "test.mutation_admin",
        json!([
            {"resource":"actions","actions":["read","create","update","delete","execute"]},
            {"resource":"rules","actions":["read","create","update","delete"]},
            {"resource":"queues","actions":["read","create","update","delete"]},
            {"resource":"triggers","actions":["read","update"]},
            {"resource":"packs","actions":["read"]}
        ]),
    )
    .await?;
    // Visibility alone must not become mutation authority. The sponsor covers
    // all operations, while this selected set grants mutations on other refs.
    assign(&ctx, identity.id, "test.mutation_narrow", json!([
        {"resource":"triggers","actions":["read"]},
        {"resource":"actions","actions":["create","update","delete"],"constraints":{"refs":["other.action"]}},
        {"resource":"rules","actions":["create","delete"],"constraints":{"refs":["other.rule"]}},
        {"resource":"queues","actions":["create"],"constraints":{"refs":["other.queue"]}}
    ])).await?;
    assign(&ctx, identity.id, "test.mutation_target", json!([
        {"resource":"actions","actions":["create","update","delete"],"constraints":{"refs":["mutation_authz.target","mutation_authz.new"]}},
        {"resource":"rules","actions":["create","delete"],"constraints":{"refs":["mutation_authz.rule"]}},
        {"resource":"queues","actions":["create"],"constraints":{"refs":["mutation_authz.queue"]}},
        {"resource":"triggers","actions":["read","update"],"constraints":{"refs":["mutation_authz.trigger"]}},
        {"resource":"packs","actions":["read"]}
    ])).await?;
    assign(
        &ctx,
        identity.id,
        "test.mutation_trigger_id",
        json!([
            {"resource":"triggers","actions":["read","update"],"constraints":{"ids":[trigger.id]}}
        ]),
    )
    .await?;
    let trigger_id_token =
        helpers::delegated_source::token(&ctx, identity.id, &["test.mutation_trigger_id".into()])
            .await?;
    let adjacent_trigger =
        create_test_trigger(&ctx.pool, pack.id, "mutation_authz.adjacent").await?;
    let narrow =
        helpers::delegated_source::token(&ctx, identity.id, &["test.mutation_narrow".into()])
            .await?;
    let target =
        helpers::delegated_source::token(&ctx, identity.id, &["test.mutation_target".into()])
            .await?;
    let action_body = json!({"ref":"mutation_authz.new","pack_ref":pack.r#ref,
        "label":"New action","entrypoint":"new.py"});
    let rule_body = json!({"ref":"mutation_authz.rule","pack_ref":pack.r#ref,
        "label":"New rule","action_ref":action.r#ref,"trigger_ref":trigger.r#ref,
        "action_params":{},"trigger_params":{}});
    let queue_body = json!({"ref":"mutation_authz.queue","pack_ref":pack.r#ref,
        "label":"New queue","dispatch_action_ref":action.r#ref});
    let worker = attune_common::auth::jwt::generate_worker_token(
        identity.id,
        "mutation-worker",
        &ctx.state.jwt_config,
        Some(600),
    )?;
    for (method, path, body) in [
        ("post", "/api/v1/actions", action_body.clone()),
        ("post", "/api/v1/rules", rule_body.clone()),
        ("post", "/api/v1/queues", queue_body.clone()),
        (
            "put",
            "/api/v1/actions/mutation_authz.target",
            json!({"label":"Worker change"}),
        ),
        ("delete", "/api/v1/actions/mutation_authz.target", json!({})),
        ("delete", "/api/v1/rules/mutation_authz.rule", json!({})),
        (
            "post",
            "/api/v1/triggers/mutation_authz.trigger/webhooks/enable",
            json!({}),
        ),
        (
            "post",
            "/api/v1/triggers/mutation_authz.trigger/webhooks/regenerate",
            json!({}),
        ),
        (
            "post",
            "/api/v1/triggers/mutation_authz.trigger/webhooks/disable",
            json!({}),
        ),
    ] {
        let response = match method {
            "put" => ctx.put(path, body, Some(&worker)).await?,
            "delete" => ctx.delete(path, Some(&worker)).await?,
            _ => ctx.post(path, body, Some(&worker)).await?,
        };
        response.assert_status(StatusCode::FORBIDDEN);
    }
    for (path, body) in [
        ("/api/v1/actions", &action_body),
        ("/api/v1/rules", &rule_body),
        ("/api/v1/queues", &queue_body),
    ] {
        ctx.post(path, body, Some(&narrow))
            .await?
            .assert_status(StatusCode::FORBIDDEN);
    }
    assert!(
        ActionRepository::find_by_ref(&ctx.pool, "mutation_authz.new")
            .await?
            .is_none()
    );
    assert!(
        RuleRepository::find_by_ref(&ctx.pool, "mutation_authz.rule")
            .await?
            .is_none()
    );
    assert!(
        WorkQueueRepository::find_by_ref(&ctx.pool, "mutation_authz.queue")
            .await?
            .is_none()
    );
    ctx.put(
        "/api/v1/actions/mutation_authz.target",
        json!({"label":"Forbidden change"}),
        Some(&narrow),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        ActionRepository::find_by_ref(&ctx.pool, &action.r#ref)
            .await?
            .unwrap()
            .label,
        action.label
    );
    ctx.delete("/api/v1/actions/mutation_authz.target", Some(&narrow))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    assert!(ActionRepository::find_by_ref(&ctx.pool, &action.r#ref)
        .await?
        .is_some());

    // Rules establish durable human sponsorship, which v1 cannot cap to an
    // execution credential. Even target Create permission must not admit one.
    ctx.post("/api/v1/rules", &rule_body, Some(&target))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    let access = attune_common::auth::jwt::generate_access_token(
        identity.id,
        "mutation_admin",
        &ctx.state.jwt_config,
    )?;
    ctx.post("/api/v1/rules", &rule_body, Some(&access))
        .await?
        .assert_status(StatusCode::CREATED);
    for (path, body) in [
        ("/api/v1/actions", &action_body),
        ("/api/v1/queues", &queue_body),
    ] {
        let response = ctx.post(path, body, Some(&target)).await?;
        let status = response.status();
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{path}: {:?}",
            response.json::<Value>().await?
        );
    }
    ctx.put(
        "/api/v1/actions/mutation_authz.target",
        json!({"label":"Authorized change"}),
        Some(&target),
    )
    .await?
    .assert_status(StatusCode::OK);
    assert_eq!(
        ActionRepository::find_by_ref(&ctx.pool, &action.r#ref)
            .await?
            .unwrap()
            .label,
        "Authorized change"
    );
    ctx.delete("/api/v1/rules/mutation_authz.rule", Some(&narrow))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    assert!(
        RuleRepository::find_by_ref(&ctx.pool, "mutation_authz.rule")
            .await?
            .is_some()
    );
    ctx.delete("/api/v1/rules/mutation_authz.rule", Some(&target))
        .await?
        .assert_status(StatusCode::OK);
    assert!(
        RuleRepository::find_by_ref(&ctx.pool, "mutation_authz.rule")
            .await?
            .is_none()
    );

    for operation in ["enable", "regenerate", "disable"] {
        let before = TriggerRepository::find_by_ref(&ctx.pool, &trigger.r#ref)
            .await?
            .unwrap();
        let path = format!("/api/v1/triggers/{}/webhooks/{operation}", trigger.r#ref);
        ctx.post(&path, json!({}), Some(&narrow))
            .await?
            .assert_status(StatusCode::FORBIDDEN);
        let unchanged = TriggerRepository::find_by_ref(&ctx.pool, &trigger.r#ref)
            .await?
            .unwrap();
        assert_eq!(unchanged.webhook_enabled, before.webhook_enabled);
        assert_eq!(unchanged.webhook_key, before.webhook_key);
        ctx.post(&path, json!({}), Some(&target))
            .await?
            .assert_status(StatusCode::OK);
        let changed = TriggerRepository::find_by_ref(&ctx.pool, &trigger.r#ref)
            .await?
            .unwrap();
        assert_eq!(changed.webhook_enabled, operation != "disable");
        if operation == "regenerate" {
            assert_ne!(changed.webhook_key, before.webhook_key);
        }
    }
    // IDs select the actual trigger, while association pack metadata must
    // neither widen the grant nor prevent a correctly scoped mutation.
    for operation in ["enable", "regenerate", "disable"] {
        let path = format!("/api/v1/triggers/{}/webhooks/{operation}", trigger.r#ref);
        ctx.post(&path, json!({}), Some(&trigger_id_token))
            .await?
            .assert_status(StatusCode::OK);
        let before = TriggerRepository::find_by_ref(&ctx.pool, &adjacent_trigger.r#ref)
            .await?
            .unwrap();
        let adjacent_path = format!(
            "/api/v1/triggers/{}/webhooks/{operation}",
            adjacent_trigger.r#ref,
        );
        ctx.post(&adjacent_path, json!({}), Some(&trigger_id_token))
            .await?
            .assert_status(StatusCode::FORBIDDEN);
        let after = TriggerRepository::find_by_ref(&ctx.pool, &adjacent_trigger.r#ref)
            .await?
            .unwrap();
        assert_eq!(before.webhook_enabled, after.webhook_enabled);
        assert_eq!(before.webhook_key, after.webhook_key);
    }
    ctx.delete("/api/v1/actions/mutation_authz.new", Some(&target))
        .await?
        .assert_status(StatusCode::OK);
    assert!(
        ActionRepository::find_by_ref(&ctx.pool, "mutation_authz.new")
            .await?
            .is_none()
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn rule_creation_cannot_turn_execution_credentials_into_durable_sponsors() -> Result<()> {
    use attune_common::repositories::action::UpdateActionInput;

    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "create_ceiling").await?;
    let action =
        create_test_action(&ctx.pool, pack.id, &pack.r#ref, "create_ceiling.target").await?;
    let trigger = create_test_trigger(&ctx.pool, pack.id, "create_ceiling.trigger").await?;
    let owner = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "create_ceiling_owner".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    // The active sponsor can execute and read every input; the live source's
    // immutable selection deliberately cannot. Create alone used to succeed.
    assign(
        &ctx,
        owner.id,
        "test.create_sponsor",
        json!([
            {"resource":"rules","actions":["create"]},
            {"resource":"actions","actions":["execute"]},
            {"resource":"packs","actions":["read","decrypt"]},
            {"resource":"keys","actions":["read","decrypt"]}
        ]),
    )
    .await?;
    assign(
        &ctx,
        owner.id,
        "test.create_only",
        json!([
            {"resource":"rules","actions":["create"],"constraints":{"pack_refs":[pack.r#ref]}}
        ]),
    )
    .await?;
    ActionRepository::update(
        &ctx.pool,
        action.id,
        UpdateActionInput {
            default_execution_permission_set_refs: Some(vec!["test.create_only".into()]),
            ..Default::default()
        },
    )
    .await?;
    let narrow =
        helpers::delegated_source::token(&ctx, owner.id, &["test.create_only".into()]).await?;
    let covered = helpers::delegated_source::token(
        &ctx,
        owner.id,
        &["test.create_sponsor".into(), "standard".into()],
    )
    .await?;
    let before: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rule), (SELECT count(*) FROM enforcement), (SELECT count(*) FROM execution)",
    ).fetch_one(&ctx.pool).await?;
    for token in [&narrow, &covered] {
        for selection in [
            None,
            Some(json!([])),
            Some(json!(["test.create_only"])),
            Some(json!(["standard"])),
        ] {
            for params in [
                json!({}),
                json!({"value":"{{ pack.config.region }}"}),
                json!({"value":"{{ keystore['system.api_token'] }}"}),
            ] {
                let mut body = json!({
                    "ref":"create_ceiling.rule", "pack_ref":pack.r#ref,
                    "label":"Cannot acquire sponsor authority", "action_ref":action.r#ref,
                    "trigger_ref":trigger.r#ref, "action_params":params, "trigger_params":{}
                });
                if let Some(selection) = &selection {
                    body["permission_set_refs"] = selection.clone();
                }
                let response = ctx.post("/api/v1/rules", body, Some(token)).await?;
                response.assert_status(StatusCode::FORBIDDEN);
                assert!(response
                    .json::<Value>()
                    .await?
                    .to_string()
                    .contains("requires an access token"));
            }
        }
    }
    let after: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rule), (SELECT count(*) FROM enforcement), (SELECT count(*) FROM execution)",
    ).fetch_one(&ctx.pool).await?;
    assert_eq!(
        before, after,
        "rejected creation must have no runtime effects"
    );
    assert!(
        RuleRepository::find_by_ref(&ctx.pool, "create_ceiling.rule")
            .await?
            .is_none()
    );
    ctx.cleanup().await?;
    Ok(())
}
