mod helpers;

use attune_common::{
    auth::jwt::{generate_access_token, JwtConfig},
    repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        rule::{CreateRuleInput, RuleRepository},
        runtime::{CreateRuntimeInput, RuntimeRepository},
        trigger::{SensorRepository, TriggerRepository, UpdateTriggerInput},
        workflow::WorkflowDefinitionRepository,
        Create, FindByRef, Patch,
    },
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

async fn token(ctx: &TestContext, name: &str, grants: Value) -> Result<String> {
    let identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: name.into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    if grants != json!([]) {
        let set = PermissionSetRepository::create(
            &ctx.pool,
            CreatePermissionSetInput {
                r#ref: format!("test.{name}"),
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
                identity: identity.id,
                permset: set.id,
            },
        )
        .await?;
    }
    Ok(generate_access_token(
        identity.id,
        name,
        &JwtConfig {
            secret: "test-secret-for-testing-only-not-secure".into(),
            access_token_expiration: 3600,
            refresh_token_expiration: 604800,
        },
    )?)
}

#[tokio::test]
async fn lifecycle_routes_deny_before_effects_and_respect_scopes() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let creator = token(
        &ctx,
        "lifecycle_creator",
        json!([
            {"resource":"actions","actions":["create","update","delete"]},
            {"resource":"triggers","actions":["create","update","delete"]},
            {"resource":"rules","actions":["update"]},
            {"resource":"packs","actions":["read","install"]}
        ]),
    )
    .await?;
    let pack = helpers::create_test_pack(&ctx.pool, "lifecycle").await?;
    let trigger = helpers::create_test_trigger(&ctx.pool, pack.id, "lifecycle.trigger").await?;
    let action =
        helpers::create_test_action(&ctx.pool, pack.id, &pack.r#ref, "lifecycle.action").await?;
    let workflow =
        helpers::create_test_workflow(&ctx.pool, pack.id, &pack.r#ref, "lifecycle.workflow")
            .await?;
    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: "lifecycle.native".into(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
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
    let sensor_request = json!({"ref":"lifecycle.sensor", "pack_ref":"lifecycle", "label":"Sensor", "entrypoint":"sensor.sh", "runtime_ref":runtime.r#ref, "trigger_ref":trigger.r#ref, "enabled":false});
    let seeded = ctx
        .post("/api/v1/sensors", sensor_request.clone(), Some(&creator))
        .await?;
    assert_eq!(
        seeded.status(),
        StatusCode::CREATED,
        "{}",
        seeded.text().await?
    );
    let sensor = SensorRepository::find_by_ref(&ctx.pool, "lifecycle.sensor")
        .await?
        .unwrap();
    TriggerRepository::update(
        &ctx.pool,
        trigger.id,
        UpdateTriggerInput {
            sensor: Some(Patch::Set(sensor.id)),
            ..Default::default()
        },
    )
    .await?;
    let rule = RuleRepository::create(
        &ctx.pool,
        CreateRuleInput {
            r#ref: "lifecycle.rule".into(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Rule".into(),
            description: None,
            action: action.id,
            action_ref: action.r#ref,
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: false,
            is_adhoc: true,
            owner_identity: None,
        },
    )
    .await?;
    let original_trigger = TriggerRepository::find_by_ref(&ctx.pool, &trigger.r#ref)
        .await?
        .unwrap();
    let original_sensor = SensorRepository::find_by_ref(&ctx.pool, "lifecycle.sensor")
        .await?
        .unwrap();
    let denied = token(&ctx, "lifecycle_denied", json!([])).await?;
    let wrong = token(&ctx, "lifecycle_wrong", json!([
        {"resource":"actions","actions":["create","update","delete"],"constraints":{"refs":["other.unrelated"]}},
        {"resource":"triggers","actions":["create","update","delete"],"constraints":{"refs":["other.unrelated"]}},
        {"resource":"rules","actions":["update"],"constraints":{"refs":["other.unrelated"]}},
        {"resource":"packs","actions":["read","install"],"constraints":{"refs":["other"]}}
    ])).await?;
    let file = json!({"name":"newfile","pack_ref":"lifecycle","label":"File", "version":"1.0.0", "definition":{"tasks":[{"name":"run","action":"lifecycle.action"}]}});
    for bearer in [&denied, &wrong] {
        for (path, body) in [
            (
                "/api/v1/workflows",
                json!({"ref":"lifecycle.new", "pack_ref":"lifecycle", "label":"New", "version":"1.0.0", "definition":{"tasks":[]}}),
            ),
            ("/api/v1/packs/lifecycle/workflow-files", file.clone()),
            (
                "/api/v1/triggers",
                json!({"ref":"lifecycle.new", "pack_ref":"lifecycle", "label":"New"}),
            ),
            ("/api/v1/sensors", {
                let mut v = sensor_request.clone();
                v["ref"] = json!("lifecycle.new");
                v
            }),
            ("/api/v1/packs/lifecycle/workflows/sync", json!({})),
            ("/api/v1/packs/lifecycle/workflows/validate", json!({})),
            (
                "/api/v1/packs/dependencies",
                json!({"pack_paths":["/unreadable-must-not-be-inspected"]}),
            ),
            (
                "/api/v1/packs/build-envs",
                json!({"pack_paths":["/unreadable-must-not-be-executed"]}),
            ),
        ] {
            let response = ctx.post(path, body, Some(bearer)).await?;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{path}: {}",
                response.text().await?
            );
        }
        for entity in [
            "triggers/lifecycle.trigger",
            "sensors/lifecycle.sensor",
            "rules/lifecycle.rule",
        ] {
            for toggle in ["enable", "disable"] {
                let path = format!("/api/v1/{entity}/{toggle}");
                let response = ctx.post(&path, json!({}), Some(bearer)).await?;
                assert_eq!(
                    response.status(),
                    StatusCode::FORBIDDEN,
                    "{path}: {}",
                    response.text().await?
                );
            }
        }
        for path in [
            "/api/v1/workflows/lifecycle.workflow",
            "/api/v1/triggers/lifecycle.trigger",
            "/api/v1/sensors/lifecycle.sensor",
        ] {
            assert_eq!(
                ctx.delete(path, Some(bearer)).await?.status(),
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        for (path, body) in [
            (
                "/api/v1/workflows/lifecycle.workflow",
                json!({"label":"Denied"}),
            ),
            (
                "/api/v1/triggers/lifecycle.trigger",
                json!({"label":"Denied"}),
            ),
            (
                "/api/v1/sensors/lifecycle.sensor",
                json!({"label":"Denied"}),
            ),
            ("/api/v1/rules/lifecycle.rule", json!({"label":"Denied"})),
        ] {
            assert_eq!(
                ctx.put(path, body, Some(bearer)).await?.status(),
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
    }
    assert!(!ctx.test_packs_dir.join("lifecycle/actions").exists());
    assert!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "lifecycle.new")
            .await?
            .is_none()
    );
    assert!(TriggerRepository::find_by_ref(&ctx.pool, "lifecycle.new")
        .await?
        .is_none());
    assert!(SensorRepository::find_by_ref(&ctx.pool, "lifecycle.new")
        .await?
        .is_none());
    assert_eq!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, &workflow.r#ref)
            .await?
            .unwrap()
            .label,
        workflow.label
    );
    assert_eq!(
        RuleRepository::find_by_ref(&ctx.pool, &rule.r#ref)
            .await?
            .unwrap()
            .enabled,
        rule.enabled
    );
    assert_eq!(
        TriggerRepository::find_by_ref(&ctx.pool, &trigger.r#ref)
            .await?
            .unwrap()
            .enabled,
        trigger.enabled
    );
    assert!(
        !SensorRepository::find_by_ref(&ctx.pool, "lifecycle.sensor")
            .await?
            .unwrap()
            .enabled
    );
    let owner = IdentityRepository::find_by_login(&ctx.pool, "lifecycle_creator")
        .await?
        .unwrap();
    let delegated =
        helpers::delegated_source::token(&ctx, owner.id, &["test.lifecycle_creator".into()])
            .await?;
    let worker = attune_common::auth::jwt::generate_worker_token(
        owner.id,
        "lifecycle-worker",
        &ctx.state.jwt_config,
        Some(600),
    )?;
    let sensor_token = attune_common::auth::jwt::generate_sensor_token(
        owner.id,
        "lifecycle.sensor",
        vec![trigger.r#ref.clone()],
        &ctx.state.jwt_config,
        Some(600),
    )?;
    // Recorded runtime authority is disabled; missing admission must fail closed in middleware.
    let recorded = attune_common::auth::jwt::generate_admitted_execution_token(
        &attune_common::repositories::runtime_authority::RuntimeCredentialBinding {
            execution_id: 9_999_991,
            admission_id: 9_999_992,
            sponsor_identity_id: owner.id,
            action_ref: "lifecycle.action".into(),
            attempt: uuid::Uuid::new_v4(),
        },
        &["test.lifecycle_creator".into()],
        &ctx.state.jwt_config,
        Some(600),
    )?;
    for (bearer, expected) in [
        (&delegated, StatusCode::FORBIDDEN),
        (&worker, StatusCode::FORBIDDEN),
        (&sensor_token, StatusCode::UNAUTHORIZED),
        (&recorded, StatusCode::UNAUTHORIZED),
    ] {
        for (path, body) in [
            (
                "/api/v1/triggers",
                json!({"ref":"lifecycle.credential", "pack_ref":"lifecycle", "label":"Denied"}),
            ),
            ("/api/v1/sensors", {
                let mut value = sensor_request.clone();
                value["ref"] = json!("lifecycle.credential");
                value
            }),
            (
                "/api/v1/workflows",
                json!({"ref":"lifecycle.credential", "pack_ref":"lifecycle", "label":"Denied", "version":"1.0.0", "definition":{"tasks":[]}}),
            ),
            ("/api/v1/packs/lifecycle/workflow-files", file.clone()),
            ("/api/v1/packs/lifecycle/workflows/sync", json!({})),
            ("/api/v1/packs/lifecycle/workflows/validate", json!({})),
            (
                "/api/v1/packs/dependencies",
                json!({"pack_paths":["/must-not-be-read"]}),
            ),
            (
                "/api/v1/packs/build-envs",
                json!({"pack_paths":["/must-not-be-executed"]}),
            ),
        ] {
            let response = ctx.post(path, body, Some(bearer)).await?;
            assert_eq!(
                response.status(),
                expected,
                "{path}: {}",
                response.text().await?
            );
        }
        for target in [
            "triggers/lifecycle.trigger",
            "sensors/lifecycle.sensor",
            "rules/lifecycle.rule",
        ] {
            for toggle in ["enable", "disable"] {
                let response = ctx
                    .post(
                        &format!("/api/v1/{target}/{toggle}"),
                        json!({}),
                        Some(bearer),
                    )
                    .await?;
                assert_eq!(
                    response.status(),
                    expected,
                    "{target}/{toggle}: {}",
                    response.text().await?
                );
            }
        }
        for target in [
            "triggers/lifecycle.trigger",
            "sensors/lifecycle.sensor",
            "workflows/lifecycle.workflow",
        ] {
            assert_eq!(
                ctx.delete(&format!("/api/v1/{target}"), Some(bearer))
                    .await?
                    .status(),
                expected,
                "{target}"
            );
        }
    }
    assert!(
        TriggerRepository::find_by_ref(&ctx.pool, "lifecycle.credential")
            .await?
            .is_none()
    );
    assert!(
        SensorRepository::find_by_ref(&ctx.pool, "lifecycle.credential")
            .await?
            .is_none()
    );
    assert!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "lifecycle.credential")
            .await?
            .is_none()
    );
    assert!(!ctx.test_packs_dir.join("lifecycle/actions").exists());
    assert_eq!(
        TriggerRepository::find_by_ref(&ctx.pool, &trigger.r#ref)
            .await?
            .unwrap()
            .updated,
        original_trigger.updated
    );
    assert_eq!(
        SensorRepository::find_by_ref(&ctx.pool, "lifecycle.sensor")
            .await?
            .unwrap()
            .updated,
        original_sensor.updated
    );
    assert_eq!(
        RuleRepository::find_by_ref(&ctx.pool, &rule.r#ref)
            .await?
            .unwrap()
            .updated,
        rule.updated
    );
    assert_eq!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, &workflow.r#ref)
            .await?
            .unwrap()
            .updated,
        workflow.updated
    );
    let scoped = token(&ctx, "lifecycle_scoped", json!([
        {"resource":"triggers","actions":["create","update","delete"],"constraints":{"refs":["lifecycle.allowed","lifecycle.trigger","lifecycle.sensor","lifecycle.rule","lifecycle.workflow","lifecycle.positive","lifecycle.newfile"]}},
        {"resource":"actions","actions":["create","update","delete"],"constraints":{"refs":["lifecycle.allowed","lifecycle.trigger","lifecycle.sensor","lifecycle.rule","lifecycle.workflow","lifecycle.positive","lifecycle.newfile"]}},
        {"resource":"rules","actions":["update"],"constraints":{"refs":["lifecycle.allowed","lifecycle.trigger","lifecycle.sensor","lifecycle.rule","lifecycle.workflow","lifecycle.positive","lifecycle.newfile"]}}
    ])).await?;
    let response = ctx
        .post(
            "/api/v1/triggers",
            json!({"ref":"lifecycle.allowed", "pack_ref":"lifecycle", "label":"Allowed"}),
            Some(&scoped),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "{}",
        response.text().await?
    );
    for toggle in ["disable", "enable"] {
        assert_eq!(
            ctx.post(
                &format!("/api/v1/triggers/lifecycle.allowed/{toggle}"),
                json!({}),
                Some(&scoped)
            )
            .await?
            .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        ctx.post(
            "/api/v1/rules/lifecycle.rule/disable",
            json!({}),
            Some(&scoped)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.post(
            "/api/v1/sensors/lifecycle.sensor/disable",
            json!({}),
            Some(&scoped)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.post(
            "/api/v1/rules/lifecycle.rule/enable",
            json!({}),
            Some(&scoped)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    // A scoped caller passes authorization but must still satisfy live placement admission.
    let admitted = ctx
        .post(
            "/api/v1/sensors/lifecycle.sensor/enable",
            json!({}),
            Some(&scoped),
        )
        .await?;
    assert_eq!(
        admitted.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        admitted.text().await?
    );
    assert!(
        !SensorRepository::find_by_ref(&ctx.pool, "lifecycle.sensor")
            .await?
            .unwrap()
            .enabled
    );
    assert_eq!(
        ctx.delete("/api/v1/workflows/lifecycle.workflow", Some(&scoped))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.delete("/api/v1/triggers/lifecycle.allowed", Some(&scoped))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.put(
            "/api/v1/sensors/lifecycle.sensor",
            json!({"label":"Updated"}),
            Some(&scoped)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.delete("/api/v1/sensors/lifecycle.sensor", Some(&scoped))
            .await?
            .status(),
        StatusCode::OK
    );
    let created_workflow = ctx.post("/api/v1/workflows", json!({"ref":"lifecycle.positive", "pack_ref":"lifecycle", "label":"Positive", "version":"1.0.0", "definition":{"tasks":[]}}), Some(&scoped)).await?;
    assert_eq!(
        created_workflow.status(),
        StatusCode::CREATED,
        "{}",
        created_workflow.text().await?
    );
    assert_eq!(
        ctx.put(
            "/api/v1/workflows/lifecycle.positive",
            json!({"label":"Updated"}),
            Some(&scoped)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.put(
            "/api/v1/triggers/lifecycle.trigger",
            json!({"label":"Updated"}),
            Some(&scoped)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.put(
            "/api/v1/rules/lifecycle.rule",
            json!({"label":"Updated"}),
            Some(&scoped)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    let file_response = ctx
        .post(
            "/api/v1/packs/lifecycle/workflow-files",
            file,
            Some(&scoped),
        )
        .await?;
    assert_eq!(
        file_response.status(),
        StatusCode::CREATED,
        "{}",
        file_response.text().await?
    );
    assert!(ctx
        .test_packs_dir
        .join("lifecycle/actions/workflows/newfile.workflow.yaml")
        .exists());
    let inspector = token(&ctx, "lifecycle_inspector", json!([{ "resource":"packs", "actions":["read","install"], "constraints":{"refs":["lifecycle"]}}])).await?;
    for operation in ["sync", "validate"] {
        let response = ctx
            .post(
                &format!("/api/v1/packs/lifecycle/workflows/{operation}"),
                json!({}),
                Some(&inspector),
            )
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await?
        );
    }
    let read_only = token(
        &ctx,
        "lifecycle_read_only",
        json!([{ "resource":"packs", "actions":["read"], "constraints":{"refs":["lifecycle"]}}]),
    )
    .await?;
    assert_eq!(
        ctx.post(
            "/api/v1/packs/lifecycle/workflows/sync",
            json!({}),
            Some(&read_only)
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.post(
            "/api/v1/packs/lifecycle/workflows/validate",
            json!({}),
            Some(&read_only)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    // Scoped pack rights cannot inspect arbitrary server paths before their identity is known.
    for operation in ["dependencies", "build-envs"] {
        assert_eq!(
            ctx.post(
                &format!("/api/v1/packs/{operation}"),
                json!({"pack_paths":["/must-not-be-opened"]}),
                Some(&inspector)
            )
            .await?
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    ctx.cleanup().await?;
    Ok(())
}
