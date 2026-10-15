mod helpers;

use attune_common::repositories::{
    component_configuration::ComponentConfigurationRepository,
    identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    },
    pack::PackRepository,
    Create, FindByRef,
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

#[tokio::test]
async fn sensor_configuration_replacement_encrypts_and_preserved_rebinding_requires_decrypt(
) -> Result<()> {
    use attune_common::repositories::{
        runtime::{CreateRuntimeInput, RuntimeRepository},
        trigger::SensorRepository,
    };
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token.as_deref().unwrap();
    ctx.post(
        "/api/v1/packs",
        json!({"ref":"configuration_sensor","label":"Sensor fixture","version":"1.0.0"}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::CREATED);
    RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: "configuration_sensor.python".into(),
            pack: None,
            pack_ref: None,
            description: None,
            name: "python".into(),
            aliases: vec![],
            distributions: json!({}),
            installation: None,
            execution_config: json!({"interpreter":{"binary":"python3"}}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await?;
    let schema =
        json!({"token":{"type":"string","secret":true},"interval":{"type":"integer","default":0}});
    let path = "/api/v1/sensors/configuration_sensor.reader";
    let reader_identity = attune_common::auth::jwt::validate_token(token, &ctx.state.jwt_config)?
        .sub
        .parse::<i64>()?;
    let reader = PermissionSetRepository::create(&ctx.pool,CreatePermissionSetInput {
        r#ref:"test.sensor_config_read".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"triggers","actions":["update"],"constraints":{"refs":["configuration_sensor.reader"]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: reader_identity,
            permset: reader.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(reader_identity).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    ctx.post("/api/v1/triggers",json!({"ref":"configuration_sensor.tick","pack_ref":"configuration_sensor","label":"Configuration tick"}),Some(token))
        .await?.assert_status(StatusCode::CREATED);
    let response: Value = ctx.post("/api/v1/sensors",json!({"ref":"configuration_sensor.reader","pack_ref":"configuration_sensor",
        "label":"Sensor fixture","entrypoint":"reader.py","runtime_ref":"configuration_sensor.python","trigger_ref":"configuration_sensor.tick",
        "enabled":false,"param_schema":schema,"config":{"token":"private-sensor-config"}}),Some(token))
        .await?.assert_status(StatusCode::CREATED).json().await?;
    assert!(!response.to_string().contains("private-sensor-config"));
    let sensor = SensorRepository::find_by_ref(&ctx.pool, "configuration_sensor.reader")
        .await?
        .unwrap();
    let original = ComponentConfigurationRepository::current(&ctx.pool, "sensor", sensor.id)
        .await?
        .unwrap();
    ctx.get(&format!("{path}?include_secret_values=true"), Some(token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    ctx.put(
        path,
        json!({"param_schema":{"op":"set","value":schema}}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    let response: Value = ctx
        .put(
            path,
            json!({"config":{"token":"replacement-sensor-config"}}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK)
        .json()
        .await?;
    assert!(!response.to_string().contains("replacement-sensor-config"));
    let replaced = ComponentConfigurationRepository::current(&ctx.pool, "sensor", sensor.id)
        .await?
        .unwrap();
    assert_ne!(replaced.id, original.id);
    assert_eq!(replaced.config["interval"], 0);
    ctx.put(path, json!({"config":{"token":false}}), Some(token))
        .await?
        .assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        ComponentConfigurationRepository::current(&ctx.pool, "sensor", sensor.id)
            .await?
            .unwrap()
            .id,
        replaced.id
    );
    let identity = reader_identity;
    let set = PermissionSetRepository::create(&ctx.pool,CreatePermissionSetInput {
        r#ref:"test.sensor_config_decrypt".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"triggers","actions":["decrypt"],"constraints":{"refs":[sensor.r#ref]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let response: Value = ctx
        .get(&format!("{path}?include_secret_values=true"), Some(token))
        .await?
        .assert_status(StatusCode::OK)
        .json()
        .await?;
    assert_eq!(
        response["data"]["config"]["token"],
        "replacement-sensor-config"
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn pack_configuration_is_redacted_and_plaintext_requires_explicit_decrypt() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token.as_deref().unwrap();
    let reference = "configuration_api_fixture";
    let request = json!({"ref":reference,"label":"Configuration fixture","version":"1.0.0",
        "conf_schema":{"token":{"type":"string","secret":true},"count":{"type":"integer","default":0}},
        "config":{"token":"private-api-configuration"}});
    let response: Value = ctx
        .post("/api/v1/packs", request, Some(token))
        .await?
        .assert_status(StatusCode::CREATED)
        .json()
        .await?;
    assert!(!response.to_string().contains("private-api-configuration"));
    let path = format!("/api/v1/packs/{reference}");
    let response: Value = ctx
        .get(&path, Some(token))
        .await?
        .assert_status(StatusCode::OK)
        .json()
        .await?;
    assert!(!response.to_string().contains("private-api-configuration"));
    assert_eq!(response["data"]["config"]["count"], 0);
    assert!(response["data"]["config_schema_binding"].is_object());
    ctx.get(&format!("{path}?include_secret_values=true"), Some(token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    let pack = PackRepository::find_by_ref(&ctx.pool, reference)
        .await?
        .unwrap();
    let original = ComponentConfigurationRepository::current(&ctx.pool, "pack", pack.id)
        .await?
        .unwrap();
    ctx.put(
        &path,
        json!({"conf_schema":{"token":{"type":"string","secret":true}}}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        ComponentConfigurationRepository::current(&ctx.pool, "pack", pack.id)
            .await?
            .unwrap()
            .id,
        original.id
    );
    let identity = attune_common::auth::jwt::validate_token(token, &ctx.state.jwt_config)?
        .sub
        .parse::<i64>()?;
    let grant = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref:"test.configuration_decrypt".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"packs","actions":["decrypt"],"constraints":{"refs":[reference]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: grant.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let response: Value = ctx
        .get(&format!("{path}?include_secret_values=true"), Some(token))
        .await?
        .assert_status(StatusCode::OK)
        .json()
        .await?;
    assert_eq!(
        response["data"]["config"]["token"],
        "private-api-configuration"
    );
    ctx.put(
        &path,
        json!({"conf_schema":{"token":{"type":"string","secret":true,"minLength":100}}}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        ComponentConfigurationRepository::current(&ctx.pool, "pack", pack.id)
            .await?
            .unwrap()
            .id,
        original.id
    );
    ctx.cleanup().await?;
    Ok(())
}
