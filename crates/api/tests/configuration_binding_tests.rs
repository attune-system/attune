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
async fn managed_sensor_put_readmits_code_schema_and_local_purposes_without_personal_key_grants(
) -> Result<()> {
    use attune_common::repositories::{
        component_lifecycle::PackProjectionIds,
        managed_sensor_metadata::ManagedSensorMetadataRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::{CreateRuntimeInput, RuntimeRepository},
        trigger::SensorRepository,
    };
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token.as_deref().unwrap();
    let identity = attune_common::auth::jwt::validate_token(token, &ctx.state.jwt_config)?
        .sub
        .parse::<i64>()?;
    ctx.post(
        "/api/v1/packs",
        json!({"ref":"sensor_readmission","label":"Readmission","version":"1.0.0"}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::CREATED);
    let pack = PackRepository::find_by_ref(&ctx.pool, "sensor_readmission")
        .await?
        .unwrap();
    RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: "sensor_readmission.python".into(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
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
    ctx.post(
        "/api/v1/triggers",
        json!({"ref":"sensor_readmission.tick","pack_ref":pack.r#ref,"label":"Tick"}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::CREATED);
    let control=PermissionSetRepository::create(&ctx.pool,CreatePermissionSetInput {
        r#ref:"test.sensor_readmission_control".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"triggers","actions":["update"],"constraints":{"refs":["sensor_readmission.reader"]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: control.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let local = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "sensor_readmission.inputs".into(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: None,
            description: None,
            grants: json!([{"resource":"keys","actions":["read","decrypt","use"]}]),
        },
    )
    .await?;
    let path = "/api/v1/sensors/sensor_readmission.reader";
    ctx.post("/api/v1/sensors",json!({"ref":"sensor_readmission.reader","pack_ref":pack.r#ref,"label":"Reader",
        "entrypoint":"sensors/reader.py","runtime_ref":"sensor_readmission.python","trigger_ref":"sensor_readmission.tick", "enabled":false,
        "param_schema":{"count":{"type":"integer"}},"config":{"count":1}}),Some(token)).await?.assert_status(StatusCode::CREATED);
    let sensor = SensorRepository::find_by_ref(&ctx.pool, "sensor_readmission.reader")
        .await?
        .unwrap();
    let mut tx = ctx.pool.begin().await?;
    ManagedSensorMetadataRepository::lock_mutation(&mut tx).await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "b".repeat(64),
            object_key: "packs/blobs/sensor-readmission.tar.gz".into(),
            provider_version: "e:sensor-readmission".into(),
            content_path: "/packs/.releases/sensor-readmission".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await?;
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            sensors: vec![sensor.id],
            ..Default::default()
        },
    )
    .await?;
    let old = ManagedSensorMetadataRepository::readmit(
        &mut tx,
        &sensor,
        identity,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    tx.commit().await?;
    let response:Value=ctx.put(path,json!({"entrypoint":"sensors/next.py",
        "param_schema":{"op":"set","value":{"count":{"type":"integer"},"extra":{"type":"string","default":"new-default"}}},
        "config":{"count":"{{ system.sensor.id }}","input_permission_set_refs":[local.r#ref],"signing_permission_set_refs":[local.r#ref]}}),Some(token))
        .await?.assert_status(StatusCode::OK).json().await?;
    let revision = response["data"]["managed_metadata_revision"]
        .as_i64()
        .unwrap();
    assert_ne!(old, revision);
    let mut connection = ctx.pool.acquire().await?;
    let new = ManagedSensorMetadataRepository::find(&mut connection, revision).await?;
    let retained = ManagedSensorMetadataRepository::find(&mut connection, old).await?;
    assert_eq!(new.executable.release.id, release.id);
    assert_eq!(new.executable.release.digest, release.digest);
    assert_eq!(new.executable.sensor.entrypoint, "sensors/next.py");
    assert_eq!(retained.executable.sensor.entrypoint, "sensors/reader.py");
    assert!(new.executable.input_grants.iter().all(|grant| grant
        .constraints
        .as_ref()
        .unwrap()
        .managing_pack_id
        == Some(pack.id)));
    drop(connection);
    ctx.put(path, json!({"label":"Harmless label"}), Some(token))
        .await?
        .assert_status(StatusCode::OK);
    assert_eq!(
        ManagedSensorMetadataRepository::current_revision(&ctx.pool, sensor.id).await?,
        Some(revision)
    );
    ctx.put(path, json!({"entrypoint":"../escape.py"}), Some(token))
        .await?
        .assert_status(StatusCode::BAD_REQUEST);
    ctx.put(
        path,
        json!({"config":{"input_permission_set_refs":["foreign.unapproved"]}}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        ManagedSensorMetadataRepository::current_revision(&ctx.pool, sensor.id).await?,
        Some(revision)
    );
    ctx.post(
        &format!("{path}/revoke-runtime"),
        json!({"metadata_revision":revision}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn trigger_rollout_preflight_is_protected_and_reports_only_emitter_ids() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token.as_deref().unwrap();
    let identity = attune_common::auth::jwt::validate_token(token, &ctx.state.jwt_config)?
        .sub
        .parse::<i64>()?;
    let permission = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "test.rollout_update".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([{"resource":"triggers","actions":["update"],"constraints":{"refs":["rollout_api.tick"]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: permission.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    ctx.post(
        "/api/v1/packs",
        json!({"ref":"rollout_api","label":"Rollout fixture","version":"1.0.0"}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::CREATED);
    ctx.post(
        "/api/v1/triggers",
        json!({"ref":"rollout_api.tick","pack_ref":"rollout_api","label":"Tick"}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::CREATED);
    let patch =
        json!({"out_schema":{"op":"set","value":{"value":{"type":"integer","required":true}}}});
    let response: Value = ctx
        .post(
            "/api/v1/triggers/rollout_api.tick/rollout-preflight",
            patch.clone(),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK)
        .json()
        .await?;
    assert_eq!(response["data"]["execution_ids"], json!([]));
    assert_eq!(response["data"]["workload_ids"], json!([]));
    assert_eq!(response["data"]["requires_acknowledgement"], false);
    assert!(response["data"].get("config").is_none());
    ctx.post(
        "/api/v1/triggers/rollout_api.tick/rollout-preflight",
        patch,
        Some("invalid"),
    )
    .await?
    .assert_status(StatusCode::UNAUTHORIZED);
    // An ordinary deployment-confirm style field is not the dedicated acknowledgement.
    let response = ctx
        .put(
            "/api/v1/triggers/rollout_api.tick",
            json!({"force":true}),
            Some(token),
        )
        .await?;
    assert!(!response.status().is_success());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn sensor_configuration_overrides_encrypt_and_equivalent_schema_reuses_binding() -> Result<()>
{
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
    .assert_status(StatusCode::OK);
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
    // Metadata-only edits do not require coverage of unrelated input/Key grants.
    ctx.put(path, json!({"label":"Harmless label"}), Some(token))
        .await?
        .assert_status(StatusCode::OK);
    for field in [
        "input_permission_set_refs",
        "cache_permission_set_refs",
        "signing_permission_set_refs",
    ] {
        ctx.put(
            path,
            json!({"config":{(field):["test.unadmitted"]}}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::BAD_REQUEST);
    }
    ctx.put(path, json!({"config":{"interval":5}}), Some(token))
        .await?
        .assert_status(StatusCode::OK);
    let response: Value = ctx
        .put(
            path,
            json!({"config_reset_fields":["interval"]}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK)
        .json()
        .await?;
    assert_eq!(response["data"]["config"]["interval"], 0);
    let after_reset = ComponentConfigurationRepository::current(&ctx.pool, "sensor", sensor.id)
        .await?
        .unwrap();
    ctx.put(path, json!({"config":{"token":false}}), Some(token))
        .await?
        .assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        ComponentConfigurationRepository::current(&ctx.pool, "sensor", sensor.id)
            .await?
            .unwrap()
            .id,
        after_reset.id
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
