mod helpers;

use attune_common::{
    auth::jwt::{generate_execution_token_with_permission_sets, validate_token, JwtConfig},
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository, UpdateIdentityInput,
        },
        key::{CreateKeyInput, KeyRepository},
        pack::{PackRepository, UpdatePackInput},
        Create, FindById, FindByRef, Update,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{
        prepare_secret_values, redaction_marker, SecretValueInput, ENTITY_EXECUTION_CONFIG,
    },
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

#[tokio::test]
async fn key_api_binds_ciphertext_to_identity_across_rename_update_and_encryption_toggles(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = validate_token(ctx.token().unwrap(), &jwt_config(&ctx))?
        .sub
        .parse::<i64>()?;
    grant(
        &ctx,
        identity,
        "fixture.key_isolation",
        json!([
            {"resource":"keys","actions":["create","read","decrypt","update"],
             "constraints":{"refs":["system.isolation_a","system.isolation_b"]}}
        ]),
    )
    .await?;
    let value = json!({"credential":"synthetic-api-only"});
    for local in ["isolation_a", "isolation_b"] {
        let response = ctx
            .post(
                "/api/v1/keys",
                json!({"local_ref":local,"owner_type":"system",
            "name":local,"value":value,"encrypted":true}),
                ctx.token(),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.json::<Value>().await?["data"]["value"], value);
    }
    let a = KeyRepository::find_by_ref(&ctx.pool, "system.isolation_a")
        .await?
        .unwrap();
    let b = KeyRepository::find_by_ref(&ctx.pool, "system.isolation_b")
        .await?
        .unwrap();
    assert_ne!(a.id, b.id);
    assert!(
        a.encryption_key_hash != b.encryption_key_hash,
        "Records must have distinct verifiers"
    );
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    assert_eq!(
        attune_common::crypto::keystore::decrypt_json(&a.value, master, a.id)?,
        value
    );
    assert!(attune_common::crypto::keystore::decrypt_json(&a.value, master, b.id).is_err());
    let path = "/api/v1/keys/system.isolation_a";
    assert_eq!(
        ctx.put(path, json!({"name":"Renamed isolation key"}), ctx.token())
            .await?
            .status(),
        StatusCode::OK
    );
    let renamed = KeyRepository::find_by_ref(&ctx.pool, "system.isolation_a")
        .await?
        .unwrap();
    assert_eq!(renamed.id, a.id);
    assert_eq!(renamed.value, a.value);
    assert!(renamed.encryption_key_hash == a.encryption_key_hash);
    assert_eq!(
        ctx.get(path, ctx.token()).await?.json::<Value>().await?["data"]["value"],
        value
    );
    let updated = json!({"credential":"synthetic-updated-only"});
    assert_eq!(
        ctx.put(path, json!({"value":updated}), ctx.token())
            .await?
            .status(),
        StatusCode::OK
    );
    for encrypted in [false, true] {
        let response = ctx
            .put(path, json!({"encrypted":encrypted}), ctx.token())
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<Value>().await?["data"]["value"], updated);
        let stored = KeyRepository::find_by_ref(&ctx.pool, "system.isolation_a")
            .await?
            .unwrap();
        assert_eq!(stored.id, a.id);
        if encrypted {
            assert_eq!(
                attune_common::crypto::keystore::decrypt_json(&stored.value, master, stored.id)?,
                updated
            );
        } else {
            assert_eq!(stored.value, updated);
            assert!(stored.encryption_key_hash.is_none());
        }
    }
    // Exercise the real read boundary against a database-level ciphertext move.
    KeyRepository::update(
        &ctx.pool,
        b.id,
        attune_common::repositories::key::UpdateKeyInput {
            value: Some(a.value),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        ctx.get("/api/v1/keys/system.isolation_b", ctx.token())
            .await?
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn admitted_sensor_signing_uses_explicit_use_grants_after_installer_freeze() -> Result<()> {
    use attune_common::repositories::{
        component_configuration::ComponentConfigurationRepository,
        identity::CreateIdentityInput,
        managed_sensor_metadata::ManagedSensorMetadataRepository,
        runtime::{CreateRuntimeInput, CreateWorkerInput, RuntimeRepository, WorkerRepository},
        sensor_schema::SensorSchemaRepository,
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadRepository,
        },
        trigger::{CreateSensorInput, CreateTriggerInput, SensorRepository, TriggerRepository},
    };
    let ctx = TestContext::new().await?.with_auth().await?;
    let installer = validate_token(ctx.token().unwrap(), &jwt_config(&ctx))?
        .sub
        .parse::<i64>()?;
    let pack = helpers::create_test_pack(&ctx.pool, "sensor_signing").await?;
    PackRepository::set_installed_by(&ctx.pool, pack.id, installer).await?;
    let permission = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: format!("{}.signer_use", pack.r#ref), pack: Some(pack.id), pack_ref: Some(pack.r#ref.clone()),
        label: None, description: None,
        grants: json!([{"resource":"keys","actions":["use"],"scope":"external","constraints":{"refs":["system.sensor_signer"]}}]),
    }).await?;
    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: format!("{}.native", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            name: "native".into(),
            description: None,
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
            r#ref: format!("{}.signer", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Signer".into(),
            description: None,
            entrypoint: "signer".into(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref.clone(),
            runtime_version_constraint: None,
            enabled: true,
            param_schema: Some(json!({"credential":{"type":"string","secret":true}})),
            config: Some(json!({"signing_permission_set_refs":[permission.r#ref],"credential":"private-sensor-input"})),
            worker_selector: json!({}),
            worker_tolerations: json!({}),
            worker_affinity: json!({}),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
        },
    )
    .await?;
    let trigger = TriggerRepository::create(
        &ctx.pool,
        CreateTriggerInput {
            r#ref: format!("{}.secret_event", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Secret event".into(),
            description: None,
            enabled: true,
            param_schema: None,
            out_schema: Some(json!({"token":{"type":"string","secret":true}})),
            event_config_schema: None,
            sensor: Some(sensor.id),
            sensor_ref: Some(sensor.r#ref.clone()),
            is_adhoc: false,
            reference_visibility: attune_common::models::ActionReferenceVisibility::Private,
            reference_allowed_pack_refs: vec![],
        },
    )
    .await?;
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &attune_common::repositories::component_lifecycle::PackProjectionIds {
            runtimes: vec![runtime.id],
            sensors: vec![sensor.id],
            triggers: vec![trigger.id],
            ..Default::default()
        },
    )
    .await?;
    let key = system_key(&ctx, "sensor_signer", json!({
        "private_key_pem": include_str!("../../common/tests/fixtures/jwt_signing_test_private.pem"),
        "profiles":{"sensor":{"issuer":"approved-client","audience":"https://example.test",
            "subjects":["approved-subject"],"pack_refs":[pack.r#ref],"sensor_refs":[sensor.r#ref],"max_ttl_seconds":120}}
    })).await?;
    assert_eq!(key.owner_type, OwnerType::System);
    assert!(key.owner_pack.is_none());

    // A declaration is not consent to operate on a system-owned private key.
    let mut denied = ctx.pool.begin().await?;
    ComponentConfigurationRepository::bind_sensor(
        &mut denied,
        &sensor,
        None,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    let missing_approval = ManagedSensorMetadataRepository::readmit(
        &mut denied,
        &sensor,
        installer,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await;
    assert!(matches!(missing_approval,
        Err(attune_common::Error::PermissionDenied(ref message))
            if message.contains("External runtime scope requires separate approval")));
    denied.rollback().await?;
    assert!(
        ManagedSensorMetadataRepository::current_revision(&ctx.pool, sensor.id)
            .await?
            .is_none()
    );

    // The registrar can consent only to this approved operation on this key.
    grant(
        &ctx,
        installer,
        "fixture.sensor_signer_consent",
        json!([
            {"resource":"keys","actions":["use"],"constraints":{"refs":[key.r#ref]}}
        ]),
    )
    .await?;
    let mut deployment = ctx.pool.begin().await?;
    ComponentConfigurationRepository::bind_sensor(
        &mut deployment,
        &sensor,
        None,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    let sensor_revision = ManagedSensorMetadataRepository::readmit(
        &mut deployment,
        &sensor,
        installer,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    deployment.commit().await?;
    assert_eq!(
        ManagedSensorMetadataRepository::current_revision(&ctx.pool, sensor.id).await?,
        Some(sensor_revision)
    );
    let mut approval_reader = ctx.pool.begin().await?;
    let (approvals, approval_count) =
        attune_common::repositories::runtime_authority::RuntimeAuthorityRepository::list_approvals(
            &mut approval_reader,
            pack.id,
            10,
            0,
        )
        .await?;
    approval_reader.commit().await?;
    assert_eq!(approval_count, 1);
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].issuer_identity, installer);
    assert_eq!(approvals[0].subject_pack, pack.id);
    assert_eq!(approvals[0].grants.0.len(), 1);
    assert_eq!(
        approvals[0].grants.0[0].resource,
        attune_common::rbac::Resource::Keys
    );
    assert_eq!(
        approvals[0].grants.0[0].actions,
        vec![attune_common::rbac::Action::Use]
    );
    assert_eq!(
        approvals[0].grants.0[0]
            .constraints
            .as_ref()
            .and_then(|scope| scope.refs.as_ref()),
        Some(&vec![key.r#ref.clone()])
    );

    let identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: format!("sensor:{}", sensor.r#ref),
            display_name: None,
            password_hash: None,
            attributes: json!({"type":"sensor"}),
        },
    )
    .await?;
    let worker = WorkerRepository::create(
        &ctx.pool,
        CreateWorkerInput {
            name: "signer-worker".into(),
            worker_type: attune_common::models::WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: None,
        },
    )
    .await?;
    let lease = match SensorWorkloadRepository::acquire_or_renew(
        &ctx.pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id: worker.id,
            worker_instance: uuid::Uuid::new_v4(),
            lease_seconds: 300,
        },
    )
    .await?
    {
        AcquireSensorWorkloadOutcome::Acquired(lease) => lease,
        _ => panic!("fixture lease unavailable"),
    };
    let workload = SensorWorkloadRepository::begin_process(&ctx.pool, lease)
        .await?
        .unwrap();
    let mut tx = ctx.pool.begin().await?;
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx).await?;
    SensorSchemaRepository::admit(
        &mut tx,
        &workload,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    tx.commit().await?;
    let (_, admitted_signer, _) =
        SensorSchemaRepository::find(&ctx.pool, workload.workload_id, workload.generation)
            .await?
            .expect("original admitted generation");
    assert_eq!(
        admitted_signer.executable.signing_permission_set_refs,
        vec![permission.r#ref.clone()]
    );
    assert_eq!(admitted_signer.executable.signing_grants.len(), 1);
    let signing_grant = &admitted_signer.executable.signing_grants[0];
    assert_eq!(signing_grant.resource, attune_common::rbac::Resource::Keys);
    assert_eq!(
        signing_grant.actions,
        vec![attune_common::rbac::Action::Use]
    );
    assert_eq!(
        signing_grant
            .constraints
            .as_ref()
            .and_then(|scope| scope.refs.as_ref()),
        Some(&vec![key.r#ref.clone()])
    );
    assert!(admitted_signer.executable.input_grants.is_empty());
    IdentityRepository::update(
        &ctx.pool,
        installer,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    sqlx::query("UPDATE permission_set SET grants = '[]' WHERE id = $1")
        .bind(permission.id)
        .execute(&ctx.pool)
        .await?;
    let token =
        attune_common::auth::jwt::generate_sensor_token_with_cache_authority_and_workload_fence(
            identity.id,
            &sensor.r#ref,
            vec![trigger.r#ref.clone()],
            Some(&pack.r#ref),
            &[],
            &[],
            workload.fence(),
            &jwt_config(&ctx),
            Some(300),
        )?;
    let path = format!("/api/v1/keys/{}/sign-jwt", key.r#ref);
    let response = ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref":trigger.r#ref,"payload":{"token":"private-emitted-value"}}),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::CREATED);
    let event_id = response.json::<Value>().await?["data"]["id"]
        .as_i64()
        .unwrap();
    let event =
        attune_common::repositories::event::EventRepository::find_by_id(&ctx.pool, event_id)
            .await?
            .unwrap();
    ctx.post(
        &path,
        json!({"profile_ref":"sensor","subject":"approved-subject","ttl_seconds":60}),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.post(
        &path,
        json!({"profile_ref":"sensor","subject":"unapproved-subject","ttl_seconds":60}),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    ctx.get(&format!("/api/v1/keys/{}", key.r#ref), Some(&token))
        .await?
        .assert_status(StatusCode::NOT_FOUND);
    let artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: format!("sensor.{}.stdout", sensor.r#ref),
            scope: OwnerType::Sensor,
            owner: sensor.r#ref.clone(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 4,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    let (binding_id, mut log_policy_fixture, _) =
        SensorSchemaRepository::find(&ctx.pool, workload.workload_id, workload.generation)
            .await?
            .unwrap();
    log_policy_fixture.triggers.clear();
    log_policy_fixture.config = attune_common::value_schema::compile(
        log_policy_fixture.config.source.clone(),
        attune_common::value_schema::SchemaContext::FlatDocument,
        &json!({}),
        &Default::default(),
        Default::default(),
    )?;
    assert!(!log_policy_fixture.declares_secrets()?);
    assert!(log_policy_fixture.requires_protected_logs()?);
    let mut tx = ctx.pool.begin().await?;
    let version = ArtifactVersionRepository::create_file_backed_with_conn(
        &mut tx,
        artifact.id,
        &artifact.r#ref,
        "text/plain".into(),
        None,
        None,
        Some("sensor".into()),
    )
    .await?;
    SensorSchemaRepository::attach_log_version(&mut tx, version.id, binding_id).await?;
    tx.commit().await?;
    let reader = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "sensor-log-reader".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    grant(
        &ctx,
        reader.id,
        "fixture.sensor_log_read",
        json!([{"resource":"triggers","actions":["update"]}, {"resource":"events","actions":["read","decrypt"]},
            {"resource":"artifacts","actions":["read"]}, {"resource":"packs","actions":["read"]}]),
    )
    .await?;
    let read_token = attune_common::auth::jwt::generate_token(
        reader.id,
        &reader.login,
        &jwt_config(&ctx),
        attune_common::auth::jwt::TokenType::Access,
    )?;
    let read_user = attune_api::auth::middleware::AuthenticatedUser {
        claims: validate_token(&read_token, &jwt_config(&ctx))?,
    };
    assert!(attune_api::secret_disclosure::authorize_sensor_runtime_log(
        &ctx.state,
        &read_user,
        version.id,
        &sensor.r#ref
    )
    .await
    .is_err());
    let hidden = attune_api::secret_disclosure::reveal_authorized(
        &ctx.state,
        &read_user,
        event.payload.clone(),
        attune_common::secret_values::ENTITY_EVENT_PAYLOAD,
        event_id,
    )
    .await?
    .unwrap();
    assert_ne!(hidden["token"], "private-emitted-value");
    let sensor_log_path = format!("/api/v1/sensors/{}/logs/stdout", sensor.r#ref);
    let artifact_log_path = format!("/api/v1/artifacts/{}/download", artifact.id);
    let artifact_stream_path = format!("/api/v1/artifacts/{}/stream", artifact.id);
    let refresh_token = attune_common::auth::jwt::generate_token(
        reader.id,
        &reader.login,
        &jwt_config(&ctx),
        attune_common::auth::jwt::TokenType::Refresh,
    )?;
    ctx.get(&artifact_stream_path, Some(&refresh_token))
        .await?
        .assert_status(StatusCode::UNAUTHORIZED);
    // Artifact delivery masks selected-source disclosure denials as 404,
    // including sensor-backed versions relabeled as general artifacts. The
    // dedicated sensor-log endpoint retains its explicit 403 contract.
    ctx.get(&artifact_stream_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::NOT_FOUND);
    ctx.get(&sensor_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    ctx.get(&artifact_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::NOT_FOUND);
    ArtifactRepository::update(
        &ctx.pool,
        artifact.id,
        attune_common::repositories::artifact::UpdateArtifactInput {
            r#ref: Some(format!("{}.ordinary_log", pack.r#ref)),
            scope: Some(OwnerType::Pack),
            owner: Some(pack.r#ref.clone()),
            classification: Some(ArtifactClassification::General),
            ..Default::default()
        },
    )
    .await?;
    ctx.get(&artifact_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::NOT_FOUND);
    ArtifactRepository::update(
        &ctx.pool,
        artifact.id,
        attune_common::repositories::artifact::UpdateArtifactInput {
            r#ref: Some(artifact.r#ref.clone()),
            scope: Some(OwnerType::Sensor),
            owner: Some(sensor.r#ref.clone()),
            classification: Some(ArtifactClassification::RuntimeLog),
            ..Default::default()
        },
    )
    .await?;
    let response = ctx
        .get(
            &format!("/api/v1/events/{event_id}?include_secret_values=true"),
            Some(&read_token),
        )
        .await?
        .assert_status(StatusCode::OK);
    assert_ne!(
        response.json::<Value>().await?["data"]["payload"]["token"],
        "private-emitted-value"
    );
    grant(
        &ctx,
        reader.id,
        "fixture.sensor_log_decrypt",
        json!([{"resource":"triggers","actions":["decrypt"]}]),
    )
    .await?;
    attune_api::secret_disclosure::authorize_sensor_runtime_log(
        &ctx.state,
        &read_user,
        version.id,
        &sensor.r#ref,
    )
    .await?;
    let revealed = attune_api::secret_disclosure::reveal_authorized(
        &ctx.state,
        &read_user,
        event.payload.clone(),
        attune_common::secret_values::ENTITY_EVENT_PAYLOAD,
        event_id,
    )
    .await?
    .unwrap();
    assert_eq!(revealed["token"], "private-emitted-value");
    ctx.get(&sensor_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::OK);
    ctx.get(&artifact_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::OK);
    let response = ctx
        .get(
            &format!("/api/v1/events/{event_id}?include_secret_values=true"),
            Some(&read_token),
        )
        .await?
        .assert_status(StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await?["data"]["payload"]["token"],
        "private-emitted-value"
    );
    assert!(attune_api::secret_disclosure::authorize_sensor_runtime_log(
        &ctx.state,
        &read_user,
        version.id,
        "other.sensor"
    )
    .await
    .is_err());
    let log_path = std::path::Path::new(&ctx.state.config.artifacts_dir)
        .join(version.file_path.as_ref().unwrap());
    tokio::fs::create_dir_all(log_path.parent().unwrap()).await?;
    tokio::fs::write(&log_path, b"runtime-sdk-secret-fixture\n").await?;
    let sdk_guarded_download = ctx
        .get(&sensor_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::OK)
        .into_response();
    let sdk_guarded_artifact_download = ctx
        .get(&artifact_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::OK)
        .into_response();
    let sdk_guarded_stream = ctx
        .get(&artifact_stream_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::OK)
        .into_response();
    let encryption = ctx.state.config.security.encryption_key.as_ref().unwrap();
    let sdk_key = KeyRepository::create_encrypted_in_pool(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "runtime_sdk".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(pack.id),
            owner_pack_ref: Some(pack.r#ref.clone()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Runtime SDK fixture".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: json!("runtime-sdk-secret-fixture"),
        },
        encryption,
    )
    .await?;
    // Implicit pack access remains available independently of installer access.
    for _ in 0..2 {
        let response: Value = ctx
            .get(&format!("/api/v1/keys/{}", sdk_key.r#ref), Some(&token))
            .await?
            .assert_status(StatusCode::OK)
            .json()
            .await?;
        assert!(response["data"]["value"] == "runtime-sdk-secret-fixture");
    }
    let origins = SensorSchemaRepository::key_read_origins(&ctx.pool, binding_id).await?;
    assert_eq!(
        origins,
        vec![attune_common::key_access::key_origin(&sdk_key)]
    );
    assert!(sqlx::query("UPDATE sensor_generation_key_read_origin SET origin = origin WHERE generation_binding = $1")
        .bind(binding_id).execute(&ctx.pool).await.is_err());
    let mut oversized_origin = origins[0].clone();
    oversized_origin.key_ref = "x".repeat(16385);
    let mut origin_transaction = ctx.pool.begin().await?;
    assert!(SensorSchemaRepository::record_key_read_origin(
        &mut origin_transaction,
        binding_id,
        &oversized_origin
    )
    .await
    .is_err());
    origin_transaction.rollback().await?;
    assert_eq!(
        SensorSchemaRepository::key_read_origins(&ctx.pool, binding_id).await?,
        origins
    );
    let revoked_download = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        axum::body::to_bytes(sdk_guarded_download.into_body(), 64 * 1024),
    )
    .await?;
    assert!(
        revoked_download.is_err(),
        "Plain sensor-log downloads must stop after SDK origin authority changes"
    );
    let revoked_artifact_download = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        axum::body::to_bytes(sdk_guarded_artifact_download.into_body(), 64 * 1024),
    )
    .await?;
    assert!(
        revoked_artifact_download.is_err(),
        "Artifact downloads must stop after SDK origin authority changes"
    );
    let revoked = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        axum::body::to_bytes(sdk_guarded_stream.into_body(), 64 * 1024),
    )
    .await??;
    let revoked = String::from_utf8(revoked.to_vec())?;
    assert!(revoked.contains("artifact_stream_access_revoked"));
    assert!(!revoked.contains("runtime-sdk-secret-fixture"));
    assert!(attune_api::secret_disclosure::authorize_sensor_runtime_log(
        &ctx.state,
        &read_user,
        version.id,
        &sensor.r#ref
    )
    .await
    .is_err());
    let copied_event: Value = ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref":trigger.r#ref,
        "payload":{"token":"runtime-sdk-secret-fixture"}}),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::CREATED)
        .json()
        .await?;
    let copied_id = copied_event["data"]["id"].as_i64().unwrap();
    let copied =
        attune_common::repositories::event::EventRepository::find_by_id(&ctx.pool, copied_id)
            .await?
            .unwrap();
    let hidden = attune_api::secret_disclosure::reveal_authorized(
        &ctx.state,
        &read_user,
        copied.payload.clone(),
        attune_common::secret_values::ENTITY_EVENT_PAYLOAD,
        copied_id,
    )
    .await?
    .unwrap();
    assert!(hidden["token"] != "runtime-sdk-secret-fixture");
    grant(
        &ctx,
        reader.id,
        "fixture.sdk_key_read",
        json!([{"resource":"keys","actions":["read"],"constraints":{"refs":[sdk_key.r#ref]}}]),
    )
    .await?;
    assert!(attune_api::secret_disclosure::authorize_sensor_runtime_log(
        &ctx.state,
        &read_user,
        version.id,
        &sensor.r#ref
    )
    .await
    .is_err());
    grant(
        &ctx,
        reader.id,
        "fixture.sdk_key_decrypt",
        json!([{"resource":"keys","actions":["decrypt"],"constraints":{"refs":[sdk_key.r#ref]}}]),
    )
    .await?;
    attune_api::secret_disclosure::authorize_sensor_runtime_log(
        &ctx.state,
        &read_user,
        version.id,
        &sensor.r#ref,
    )
    .await?;
    let revealed = attune_api::secret_disclosure::reveal_authorized(
        &ctx.state,
        &read_user,
        copied.payload.clone(),
        attune_common::secret_values::ENTITY_EVENT_PAYLOAD,
        copied_id,
    )
    .await?
    .unwrap();
    assert!(revealed["token"] == "runtime-sdk-secret-fixture");
    assert!(SensorWorkloadRepository::release(&ctx.pool, workload.fence()).await?);
    let restarted_lease = match SensorWorkloadRepository::acquire_or_renew(
        &ctx.pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id: worker.id,
            worker_instance: uuid::Uuid::new_v4(),
            lease_seconds: 300,
        },
    )
    .await?
    {
        AcquireSensorWorkloadOutcome::Acquired(lease) => lease,
        _ => panic!("restart fixture lease unavailable"),
    };
    let restarted = SensorWorkloadRepository::begin_process(&ctx.pool, restarted_lease)
        .await?
        .unwrap();
    let mut restart_transaction = ctx.pool.begin().await?;
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(
        &mut restart_transaction,
    )
    .await?;
    SensorSchemaRepository::admit(
        &mut restart_transaction,
        &restarted,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    restart_transaction.commit().await?;
    let (restarted_binding, _, _) =
        SensorSchemaRepository::find(&ctx.pool, restarted.workload_id, restarted.generation)
            .await?
            .unwrap();
    assert_ne!(binding_id, restarted_binding);
    assert!(
        SensorSchemaRepository::key_read_origins(&ctx.pool, restarted_binding)
            .await?
            .is_empty()
    );
    ctx.get(&format!("/api/v1/keys/{}", sdk_key.r#ref), Some(&token))
        .await?
        .assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(
        SensorSchemaRepository::key_read_origins(&ctx.pool, binding_id).await?,
        origins
    );
    // Restart does not erase the disclosure authority of historical log versions.
    attune_api::secret_disclosure::authorize_sensor_runtime_log(
        &ctx.state,
        &read_user,
        version.id,
        &sensor.r#ref,
    )
    .await?;
    let restarted_token =
        attune_common::auth::jwt::generate_sensor_token_with_cache_authority_and_workload_fence(
            identity.id,
            &sensor.r#ref,
            vec![trigger.r#ref.clone()],
            Some(&pack.r#ref),
            &[],
            &[],
            restarted.fence(),
            &jwt_config(&ctx),
            Some(300),
        )?;
    let restarted_read: Value = ctx
        .get(
            &format!("/api/v1/keys/{}", sdk_key.r#ref),
            Some(&restarted_token),
        )
        .await?
        .assert_status(StatusCode::OK)
        .json()
        .await?;
    assert!(restarted_read["data"]["value"] == "runtime-sdk-secret-fixture");
    assert_eq!(
        SensorSchemaRepository::key_read_origins(&ctx.pool, restarted_binding).await?,
        origins
    );
    let open_stream = ctx
        .get(&artifact_stream_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::OK)
        .into_response();
    sqlx::query("DELETE FROM sensor_generation_schema_binding WHERE id = $1")
        .bind(binding_id)
        .execute(&ctx.pool)
        .await?;
    let frames = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        axum::body::to_bytes(open_stream.into_body(), 64 * 1024),
    )
    .await??;
    let frames = String::from_utf8(frames.to_vec())?;
    assert!(frames.contains("artifact_stream_access_revoked"));
    assert!(!frames.contains("private-emitted-value"));
    assert!(
        SensorSchemaRepository::key_read_origins(&ctx.pool, binding_id)
            .await?
            .is_empty()
    );
    assert_eq!(
        SensorSchemaRepository::key_read_origins(&ctx.pool, restarted_binding).await?,
        origins
    );
    assert!(SensorWorkloadRepository::release(&ctx.pool, restarted.fence()).await?);
    ctx.get(&sensor_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    ctx.get(&artifact_log_path, Some(&read_token))
        .await?
        .assert_status(StatusCode::NOT_FOUND);
    assert!(attune_api::secret_disclosure::authorize_sensor_runtime_log(
        &ctx.state,
        &read_user,
        version.id,
        &sensor.r#ref
    )
    .await
    .is_err());
    ctx.cleanup().await?;
    Ok(())
}

fn jwt_config(ctx: &TestContext) -> JwtConfig {
    JwtConfig {
        secret: ctx.state.config.security.jwt_secret.clone().unwrap(),
        access_token_expiration: 300,
        refresh_token_expiration: 3600,
    }
}

async fn grant(ctx: &TestContext, identity: i64, reference: &str, grants: Value) -> Result<()> {
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
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    Ok(())
}

async fn signing_key(ctx: &TestContext) -> Result<attune_common::models::Key> {
    let value = json!({
        "private_key_pem": include_str!("../../common/tests/fixtures/jwt_signing_test_private.pem"),
        "profiles": {"salesforce": {"issuer":"approved-client", "audience":"https://login.salesforce.com",
            "subjects":["sales@example.com","support@example.com"], "pack_refs":["sales","support"], "max_ttl_seconds":120}}
    });
    system_key(ctx, "salesforce_signer", value).await
}

async fn system_key(
    ctx: &TestContext,
    local_ref: &str,
    value: Value,
) -> Result<attune_common::models::Key> {
    let encryption = ctx.state.config.security.encryption_key.as_ref().unwrap();
    Ok(KeyRepository::create_encrypted_in_pool(
        &ctx.pool,
        CreateKeyInput {
            local_ref: local_ref.into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: format!("{local_ref} fixture key"),
            encrypted: true,
            encryption_key_hash: None,
            value,
        },
        encryption,
    )
    .await?)
}

#[tokio::test]
async fn disclosure_uses_original_key_bindings_and_requires_every_combined_origin() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = validate_token(ctx.token().unwrap(), &jwt_config(&ctx))?
        .sub
        .parse::<i64>()?;
    grant(&ctx, identity, "fixture.key_read", json!([
        {"resource":"keys","actions":["read"],"constraints":{"owner_types":["system"],"refs":["system.original","system.replacement"]}},
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read","decrypt"]}
    ])).await?;
    let original = system_key(&ctx, "original", json!("original-fixture-only")).await?;
    let replacement = system_key(&ctx, "replacement", json!("replacement-fixture-only")).await?;
    let make_secret =
        |path: &str, value: &str, origins: Vec<SecretOrigin>| -> Result<SecretValueInput> {
            Ok(SecretValueInput {
                json_path: path.into(),
                value: json!(value),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&SecretProvenance {
                    origins,
                    templates: vec![attune_common::secret_provenance::TemplateOrigin {
                        component_type: "rule".into(),
                        component_ref: "sales.login".into(),
                        input_path: path.into(),
                        expression: "keystore[pack.config.signer_ref]".into(),
                    }],
                })?),
            })
        };
    let inputs = vec![
        make_secret(
            "/bound",
            "original-fixture-only",
            vec![SecretOrigin::Key(attune_common::key_access::key_origin(
                &original,
            ))],
        )?,
        make_secret(
            "/combined",
            "original:replacement",
            vec![
                SecretOrigin::Key(attune_common::key_access::key_origin(&original)),
                SecretOrigin::Key(attune_common::key_access::key_origin(&replacement)),
            ],
        )?,
    ];
    let prepared = prepare_secret_values(
        inputs,
        ctx.state.config.security.encryption_key.as_ref().unwrap(),
    )?;
    let id = execution_with_original_secret_fields(
        &ctx,
        "sales",
        json!({"bound":redaction_marker(), "combined":redaction_marker()}),
        &prepared,
    )
    .await?;
    // Unsupported provenance rejects the whole disclosure frontier. Keep that
    // negative separate so it cannot mask the known-origin disclosure checks.
    let unknown_fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/unknown".into(),
            value: json!("must-remain-hidden"),
            source_kind: "keystore".into(),
            source_ref: Some("system.original".into()),
        }],
        ctx.state.config.security.encryption_key.as_ref().unwrap(),
    )?;
    let unknown_id = execution_with_original_secret_fields(
        &ctx,
        "unknown_origin",
        json!({"unknown":redaction_marker()}),
        &unknown_fields,
    )
    .await?;
    let pack = PackRepository::find_by_ref(&ctx.pool, "sales")
        .await?
        .unwrap();
    PackRepository::update(
        &ctx.pool,
        pack.id,
        UpdatePackInput {
            config: Some(json!({"signer_ref":"system.replacement"})),
            ..Default::default()
        },
    )
    .await?;
    grant(
        &ctx,
        identity,
        "fixture.replacement_decrypt",
        json!([
            {"resource":"keys","actions":["decrypt"],"constraints":{"refs":["system.replacement"]}}
        ]),
    )
    .await?;
    let url = format!("/api/v1/executions/{id}?include_secret_values=true");
    let response = ctx.get(&url, ctx.token()).await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "known origins before original Key decrypt grant"
    );
    let body: Value = response.json().await?;
    for field in ["bound", "combined"] {
        assert!(attune_common::secret_values::is_redaction_marker(
            &body["data"]["config"][field]
        ));
    }
    let known = ExecutionSecretValueRepository::find_stored_by_entity(
        &ctx.pool,
        ENTITY_EXECUTION_CONFIG,
        id,
    )
    .await?;
    assert!(attune_common::key_access::authorize_parameter_key_origins(
        &ctx.pool,
        Some(identity),
        &known
    )
    .await
    .is_err());
    grant(
        &ctx,
        identity,
        "fixture.original_decrypt",
        json!([
            {"resource":"keys","actions":["decrypt"],"constraints":{"refs":["system.original"]}}
        ]),
    )
    .await?;
    attune_common::key_access::authorize_parameter_key_origins(&ctx.pool, Some(identity), &known)
        .await?;
    assert!(
        attune_common::key_access::authorize_parameter_key_origins(&ctx.pool, None, &known)
            .await
            .is_err()
    );
    let mut local = known[0].clone();
    local.source_ref = Some(serde_json::to_string(
        &attune_common::secret_values::SecretSource::ParameterSchema {
            path: local.json_path.clone(),
        }
        .provenance(),
    )?);
    attune_common::key_access::authorize_parameter_key_origins(&ctx.pool, None, &[local]).await?;
    let response = ctx.get(&url, ctx.token()).await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "known origins after both Key decrypt grants"
    );
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["config"]["bound"], "original-fixture-only");
    assert_eq!(body["data"]["config"]["combined"], "original:replacement");
    // Even both real Key grants cannot authorize an unsupported source kind.
    let response = ctx
        .get(
            &format!("/api/v1/executions/{unknown_id}?include_secret_values=true"),
            ctx.token(),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "unknown provenance disclosure"
    );
    let denied: Value = response.json().await?;
    assert_eq!(denied["code"].as_str(), Some("FORBIDDEN"));
    assert!(denied["error"].is_string());
    assert!(denied.get("data").is_none());
    let denied_json = serde_json::to_string(&denied)?;
    for plaintext in [
        "must-remain-hidden",
        "original-fixture-only",
        "replacement-fixture-only",
        "original:replacement",
    ] {
        assert!(!denied_json.contains(plaintext));
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn named_execution_token_authority_is_bounded_by_current_active_executor() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let config = jwt_config(&ctx);
    let identity = validate_token(ctx.token().unwrap(), &config)?
        .sub
        .parse::<i64>()?;
    grant(&ctx, identity, "fixture.key_read", json!([
        {"resource":"keys","actions":["read"],"constraints":{"refs":["system.salesforce_signer"]}},
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read"]}
    ])).await?;
    signing_key(&ctx).await?;
    let id = execution(&ctx, "sales", json!({})).await?;
    let mut token = generate_execution_token_with_permission_sets(
        identity,
        id,
        "sales.login",
        &config,
        Some(60),
        &["fixture.key_read".into()],
    )?;
    let response = ctx
        .get("/api/v1/keys/system.salesforce_signer", Some(&token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.escalated".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"identities","actions":["create","delete"]}]),
        },
    )
    .await?;
    token = generate_execution_token_with_permission_sets(
        identity,
        id,
        "sales.login",
        &config,
        Some(60),
        &["fixture.escalated".into()],
    )?;
    let response = ctx
        .get("/api/v1/keys/system.salesforce_signer", Some(&token))
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    IdentityRepository::update(
        &ctx.pool,
        identity,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    token = generate_execution_token_with_permission_sets(
        identity,
        id,
        "sales.login",
        &config,
        Some(60),
        &["fixture.key_read".into()],
    )?;
    let response = ctx
        .get("/api/v1/keys/system.salesforce_signer", Some(&token))
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    ctx.cleanup().await?;
    Ok(())
}

async fn execution(ctx: &TestContext, pack_ref: &str, parameters: Value) -> Result<i64> {
    let action = execution_action(ctx, pack_ref).await?;
    let response = ctx.post("/api/v1/executions/execute",json!({"action_ref":action.r#ref,"parameters":parameters,"permission_set_refs":["fixture.key_read"]}),ctx.token()).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body["data"]["schema_binding"]["input"]["annotated_schema"].is_object());
    assert!(body["data"]["schema_binding"]["input_digest"]
        .as_str()
        .is_some());
    assert!(body["data"]["schema_binding"]
        .get("static_actions")
        .is_none());
    Ok(body["data"]["id"].as_i64().unwrap())
}

async fn execution_action(
    ctx: &TestContext,
    pack_ref: &str,
) -> Result<attune_common::models::Action> {
    let pack = helpers::create_test_pack(&ctx.pool, pack_ref).await?;
    // These cases exercise active human-owned delegation, not managed admission.
    let action = attune_common::repositories::action::ActionRepository::create(
        &ctx.pool,
        attune_common::repositories::action::CreateActionInput {
            r#ref: format!("{pack_ref}.login"),
            pack: pack.id,
            pack_ref: pack_ref.into(),
            label: "Ad-hoc signing caller".into(),
            description: None,
            entrypoint: "main.py".into(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: json!({}),
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            param_schema: None,
            out_schema: None,
            is_adhoc: true,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        },
    )
    .await?;
    helpers::activate_test_pack_release(&ctx.pool, &pack).await?;
    Ok(action)
}

async fn execution_with_original_secret_fields(
    ctx: &TestContext,
    pack_ref: &str,
    redacted_config: Value,
    fields: &[attune_common::secret_values::PreparedSecretValue],
) -> Result<i64> {
    use attune_common::repositories::{
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository},
        schema_contract::{BindingOwner, SchemaContractRepository},
    };

    let action = execution_action(ctx, pack_ref).await?;
    let identity = validate_token(ctx.token().unwrap(), &jwt_config(ctx))?
        .sub
        .parse::<i64>()?;
    let mut tx = ctx.pool.begin().await?;
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id).await?;
    assert!(snapshot.executable.action.is_adhoc);
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    // These disclosure fixtures retain original Key origins, including deliberately
    // unknown provenance. They are human-owned ad-hoc inputs, not managed proofs.
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref,
            executor: Some(identity),
            permission_set_refs: vec!["fixture.key_read".into()],
            status: attune_common::models::ExecutionStatus::Requested,
            config: None,
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await?;
    let initialized = ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        execution.id,
        None,
        &snapshot,
        Some(redacted_config.clone()),
        fields,
    )
    .await?;
    assert_eq!(initialized.config, Some(redacted_config));
    let stored = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        ENTITY_EXECUTION_CONFIG,
        execution.id,
    )
    .await?;
    assert_eq!(stored.len(), fields.len());
    for field in fields {
        let actual = stored
            .iter()
            .find(|actual| actual.json_path == field.json_path)
            .unwrap();
        assert_eq!(actual.encrypted_value, field.encrypted_value);
        assert_eq!(
            actual.encryption_key_hash.as_deref(),
            Some(field.encryption_key_hash.as_str())
        );
        assert_eq!(actual.source_kind, field.source_kind);
        assert_eq!(actual.source_ref, field.source_ref);
    }
    tx.commit().await?;
    let committed = ExecutionRepository::find_by_id(&ctx.pool, execution.id)
        .await?
        .unwrap();
    assert_eq!(committed.config, initialized.config);
    assert_eq!(
        ExecutionSecretValueRepository::find_stored_by_entity(
            &ctx.pool,
            ENTITY_EXECUTION_CONFIG,
            execution.id,
        )
        .await?,
        stored,
    );
    Ok(execution.id)
}

#[tokio::test]
async fn read_only_users_can_use_approved_signer_but_cannot_retrieve_private_material() -> Result<()>
{
    let ctx = TestContext::new().await?.with_auth().await?;
    let config = jwt_config(&ctx);
    let identity = validate_token(ctx.token().unwrap(), &config)?
        .sub
        .parse::<i64>()?;
    grant(&ctx,identity,"fixture.key_read",json!([
        {"resource":"keys","actions":["read"],"constraints":{"owner_types":["system"],"refs":["system.salesforce_signer"]}},
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read","decrypt"]}
    ])).await?;
    let key = signing_key(&ctx).await?;
    let response = ctx
        .get("/api/v1/keys/system.salesforce_signer", ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(body["data"]["value"].is_null());
    for (pack_ref, subject) in [
        ("sales", "sales@example.com"),
        ("support", "support@example.com"),
    ] {
        let id = execution(&ctx, pack_ref, json!({})).await?;
        let token = generate_execution_token_with_permission_sets(
            identity,
            id,
            &format!("{pack_ref}.login"),
            &config,
            Some(60),
            &["fixture.key_read".into()],
        )?;
        let response = ctx
            .post(
                "/api/v1/keys/system.salesforce_signer/sign-jwt",
                json!({"profile_ref":"salesforce","subject":subject,"ttl_seconds":60}),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert!(
            body["data"]["assertion"]
                .as_str()
                .unwrap()
                .split('.')
                .count()
                == 3
        );
        assert!(body["data"].get("private_key_pem").is_none());
        let response = ctx
            .post(
                "/api/v1/keys/system.salesforce_signer/sign-jwt",
                json!({"profile_ref":"salesforce","subject":"admin@example.com","ttl_seconds":60}),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = ctx
            .get("/api/v1/keys/system.salesforce_signer", Some(&token))
            .await?;
        let body: Value = response.json().await?;
        assert!(body["data"]["value"].is_null());
    }
    let response = ctx
        .post(
            "/api/v1/keys/system.salesforce_signer/sign-jwt",
            json!({"profile_ref":"salesforce","subject":"sales@example.com","ttl_seconds":60}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(key.encrypted);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_decrypt_does_not_override_recorded_key_disclosure_authority() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = validate_token(ctx.token().unwrap(), &jwt_config(&ctx))?
        .sub
        .parse::<i64>()?;
    grant(&ctx,identity,"fixture.key_read",json!([
        {"resource":"keys","actions":["read"],"constraints":{"owner_types":["system"],"refs":["system.salesforce_signer"]}},
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read","decrypt"]}
    ])).await?;
    let key = signing_key(&ctx).await?;
    let provenance = SecretProvenance {
        origins: vec![SecretOrigin::Key(attune_common::key_access::key_origin(
            &key,
        ))],
        templates: Vec::new(),
    };
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/signing_key".into(),
            value: json!("dummy-private-material"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&provenance)?),
        }],
        ctx.state.config.security.encryption_key.as_ref().unwrap(),
    )?;
    let id = execution_with_original_secret_fields(
        &ctx,
        "sales",
        json!({"signing_key":redaction_marker()}),
        &prepared,
    )
    .await?;
    let response = ctx
        .get(
            &format!("/api/v1/executions/{id}?include_secret_values=true"),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(attune_common::secret_values::is_redaction_marker(
        &body["data"]["config"]["signing_key"]
    ));
    let response = ctx
        .get(
            &format!("/api/v1/executions/{id}/logs/stdout/stream"),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    grant(
        &ctx,
        identity,
        "fixture.log_read",
        json!([{"resource":"artifacts","actions":["read"]}]),
    )
    .await?;
    let artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: format!("execution.{id}.stdout"),
            scope: OwnerType::Pack,
            owner: "sales".into(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Days,
            retention_limit: 1,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact: artifact.id,
            execution: Some(id),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: Some(format!("execution/{id}/stdout/1/log.txt")),
            meta: None,
            created_by: None,
        },
    )
    .await?;
    let response = ctx
        .get(&format!("/api/v1/artifacts/{}", artifact.id), ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let header: Value = response.json().await?;
    for field in [
        "data",
        "description",
        "meta",
        "file_path",
        "content_type",
        "size_bytes",
    ] {
        assert!(
            header["data"].get(field).is_none(),
            "header exposed {field}"
        );
    }
    let response = ctx
        .get(
            &format!("/api/v1/artifacts/{}/versions/1", artifact.id),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    grant(&ctx,identity,"fixture.key_decrypt",json!([{ "resource":"keys","actions":["decrypt"],"constraints":{"owner_types":["system"],"refs":["system.salesforce_signer"]}}])).await?;
    let response = ctx
        .get(
            &format!("/api/v1/artifacts/{}/versions/1", artifact.id),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response = ctx
        .get(
            &format!("/api/v1/executions/{id}?include_secret_values=true"),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(
        body["data"]["config"]["signing_key"],
        "dummy-private-material"
    );
    assert!(KeyRepository::find_by_ref(&ctx.pool, &key.r#ref)
        .await?
        .is_some());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn key_write_authority_does_not_disclose_existing_encrypted_material() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = validate_token(ctx.token().unwrap(), &jwt_config(&ctx))?
        .sub
        .parse::<i64>()?;
    IdentityRepository::update(
        &ctx.pool,
        identity,
        UpdateIdentityInput {
            attributes: Some(json!({"team":"ops"})),
            ..Default::default()
        },
    )
    .await?;
    grant(&ctx, identity, "fixture.key_writer", json!([
        {"resource":"keys","actions":["read","update"],"constraints":{"refs":["system.probe"],"attributes":{"team":"ops"}}}
    ])).await?;
    system_key(&ctx, "probe", json!("fixture-private")).await?;
    let response = ctx
        .put(
            "/api/v1/keys/system.probe",
            json!({"name":"renamed key"}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(body["data"]["value"].is_null());
    assert!(!body.to_string().contains("fixture-private"));
    let response = ctx
        .put(
            "/api/v1/keys/system.probe",
            json!({"encrypted":false}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        KeyRepository::find_by_ref(&ctx.pool, "system.probe")
            .await?
            .unwrap()
            .encrypted
    );
    grant(
        &ctx,
        identity,
        "fixture.key_open",
        json!([
            {"resource":"keys","actions":["decrypt"],"constraints":{"refs":["system.probe"]}}
        ]),
    )
    .await?;
    for encrypted in [false, true] {
        let response = ctx
            .put(
                "/api/v1/keys/system.probe",
                json!({"encrypted":encrypted}),
                ctx.token(),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert_eq!(body["data"]["value"], "fixture-private");
        let stored = KeyRepository::find_by_ref(&ctx.pool, "system.probe")
            .await?
            .unwrap();
        assert_eq!(stored.encrypted, encrypted);
        assert_eq!(stored.encryption_key_hash.is_some(), encrypted);
    }
    ctx.cleanup().await?;
    Ok(())
}
