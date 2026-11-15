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
                let response = response.assert_status(StatusCode::FORBIDDEN);
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

/// Real broker consumers; run only against the lane-owned RabbitMQ and PG.
#[tokio::test]
#[ignore = "requires owned RabbitMQ via ATTUNE_RULE_TEST_AMQP_URL and PostgreSQL"]
async fn access_created_rule_retains_sponsor_and_inputs_through_event_consumers() -> Result<()> {
    use attune_common::{
        event_schema::EventSchemaBinding,
        mq::{
            Connection, Consumer, ConsumerConfig, EventCreatedPayload, MessageEnvelope,
            MessageQueueConfig, MessageType, Publisher, PublisherConfig,
        },
        repositories::{
            component_lifecycle::PackProjectionIds,
            delegated_execution::DelegatedExecutionRepository,
            event::{CreateEventInput, EnforcementRepository, EventRepository},
            event_origin::EventOriginRepository,
            execution::ExecutionRepository,
        },
    };
    use attune_executor::{
        enforcement_processor::EnforcementProcessor, event_processor::EventProcessor,
        ExecutionQueueManager, PolicyEnforcer, QueueConfig,
    };
    use futures::FutureExt;
    use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};

    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "access_event_sponsor").await?;
    let action = create_test_action(
        &ctx.pool,
        pack.id,
        &pack.r#ref,
        "access_event_sponsor.target",
    )
    .await?;
    let trigger = create_test_trigger(&ctx.pool, pack.id, "access_event_sponsor.tick").await?;
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![action.id],
            triggers: vec![trigger.id],
            ..Default::default()
        },
    )
    .await?;
    let owner = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "access_event_owner".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    assign(
        &ctx,
        owner.id,
        "test.access_event_owner",
        json!([
            {"resource":"rules","actions":["create"]},
            {"resource":"actions","actions":["execute"],"constraints":{"refs":[action.r#ref]}}
        ]),
    )
    .await?;
    let access = attune_common::auth::jwt::generate_access_token(
        owner.id,
        "access_event_owner",
        &ctx.state.jwt_config,
    )?;
    let response = ctx
        .post(
            "/api/v1/rules",
            json!({
                "ref":"access_event_sponsor.rule", "pack_ref":pack.r#ref,
                "label":"Access sponsor", "action_ref":action.r#ref, "trigger_ref":trigger.r#ref,
                "enabled":true, "permission_set_refs":["standard"],
                "action_params":{"message":"{{ event.payload.message }}"}, "trigger_params":{}
            }),
            Some(&access),
        )
        .await?
        .assert_status(StatusCode::CREATED);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["owner_identity"], owner.id);
    let rule = RuleRepository::find_by_ref(&ctx.pool, "access_event_sponsor.rule")
        .await?
        .unwrap();
    assert!(rule.is_adhoc);
    assert_eq!(rule.owner_identity, Some(owner.id));

    let payload = json!({"message":"authorized-event-value"});
    let mut tx = ctx.pool.begin().await?;
    let event = EventRepository::create(
        &mut *tx,
        CreateEventInput {
            trigger: Some(trigger.id),
            trigger_ref: trigger.r#ref.clone(),
            config: None,
            payload: Some(payload.clone()),
            trace_tag: None,
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        },
    )
    .await?;
    EventOriginRepository::capture(
        &mut tx,
        &event,
        &EventSchemaBinding {
            trigger_id: Some(trigger.id),
            trigger_ref: trigger.r#ref.clone(),
            payload: None,
            config: None,
            webhook: false,
        },
        &[],
        &[],
    )
    .await?;
    tx.commit().await?;

    // The broker is exclusively owned by this test lane. No other service
    // consumes these queues; messages may safely arrive before start() awaits.
    let connection = Connection::connect(&std::env::var("ATTUNE_RULE_TEST_AMQP_URL")?).await?;
    let mq = MessageQueueConfig::default();
    connection.setup_common_infrastructure(&mq).await?;
    connection.setup_executor_infrastructure(&mq).await?;
    let publisher = Arc::new(
        Publisher::new(
            &connection,
            PublisherConfig {
                confirm_publish: true,
                timeout_secs: 10,
                exchange: "attune.events".into(),
            },
        )
        .await?,
    );
    let mut consumers = Vec::new();
    for (queue, tag) in [
        (&mq.rabbitmq.queues.executor_events.name, "rule-event-test"),
        (
            &mq.rabbitmq.queues.enforcements.name,
            "rule-enforcement-test",
        ),
    ] {
        consumers.push(Arc::new(
            Consumer::new(
                &connection,
                ConsumerConfig {
                    queue: queue.clone(),
                    tag: tag.into(),
                    prefetch_count: 1,
                    auto_ack: false,
                    exclusive: false,
                },
            )
            .await?,
        ));
    }
    let event_processor = EventProcessor::new(
        ctx.pool.clone(),
        publisher.clone(),
        consumers[0].clone(),
        ctx.state.config.security.encryption_key.clone(),
    );
    let enforcement_processor = EnforcementProcessor::new(
        ctx.pool.clone(),
        publisher.clone(),
        consumers[1].clone(),
        Arc::new(PolicyEnforcer::new(ctx.pool.clone())),
        Arc::new(ExecutionQueueManager::new(QueueConfig::default())),
        ctx.state.config.security.encryption_key.clone(),
    );
    let event_task = tokio::spawn(async move { event_processor.start().await });
    let enforcement_task = tokio::spawn(async move { enforcement_processor.start().await });
    let check = async {
        publisher
            .publish_envelope(&MessageEnvelope::new(
                MessageType::EventCreated,
                EventCreatedPayload {
                    event_created: Some(event.created),
                    event_id: event.id,
                    trigger_id: Some(trigger.id),
                    trigger_ref: trigger.r#ref.clone(),
                    sensor_id: None,
                    sensor_ref: None,
                    payload: payload.clone(),
                    config: None,
                },
            ))
            .await?;
        let (enforcement, execution) = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let enforcements = EnforcementRepository::find_by_rule(&ctx.pool, rule.id).await?;
                if let Some(enforcement) = enforcements.into_iter().next() {
                    let executions =
                        ExecutionRepository::find_by_enforcement(&ctx.pool, enforcement.id).await?;
                    if let Some(execution) = executions.into_iter().next() {
                        return Ok::<_, attune_common::Error>((enforcement, execution));
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| {
            "Access-created rule did not traverse event/enforcement consumers within 30 seconds"
        })??;
        assert_eq!(enforcement.event, Some(event.id));
        assert_eq!(execution.enforcement, Some(enforcement.id));
        assert_eq!(execution.executor, Some(owner.id));
        assert_eq!(execution.action_ref, action.r#ref);
        assert_eq!(
            execution.config,
            Some(json!({"message":"authorized-event-value"}))
        );
        assert_eq!(execution.permission_set_refs, vec!["standard".to_string()]);
        let mut tx = ctx.pool.begin().await?;
        assert_eq!(
            DelegatedExecutionRepository::enforcement_selection(&mut tx, &enforcement).await?,
            (owner.id, vec!["standard".into()])
        );
        let selection = DelegatedExecutionRepository::find(&mut tx, execution.id).await?;
        assert_eq!(selection.mode, "delegated");
        assert!(selection.explicit);
        assert_eq!(selection.owner, Some(owner.id));
        assert_eq!(selection.selected_refs, execution.permission_set_refs);
        assert!(selection.admission.is_none());
        assert!(selection.executable_snapshot.is_some());
        assert!(selection.schema_digest.is_some());
        assert!(selection.grant_digest.is_some());
        tx.commit().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let result = AssertUnwindSafe(check).catch_unwind().await;
    // Always await stop and both writers before closing/removing the database,
    // including assertion/timeout failures returned from the consumer check.
    let stop_event = consumers[0].stop().await;
    let stop_enforcement = consumers[1].stop().await;
    async fn join_writer(mut task: tokio::task::JoinHandle<anyhow::Result<()>>) -> Result<()> {
        match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
            Ok(done) => {
                done??;
                Ok(())
            }
            Err(_) => {
                task.abort();
                let _cancelled = task.await;
                Err("consumer failed to stop; aborted and joined".into())
            }
        }
    }
    let event_done = join_writer(event_task).await;
    let enforcement_done = join_writer(enforcement_task).await;
    let connection_closed = connection.close().await;
    stop_event?;
    stop_enforcement?;
    event_done?;
    enforcement_done?;
    connection_closed?;
    ctx.cleanup().await?;
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
