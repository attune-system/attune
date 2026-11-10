mod helpers;

use attune_api::authz::AuthorizationService;
use attune_common::{
    models::enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, CreateArtifactInput},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        runtime::{CreateRuntimeInput, RuntimeRepository},
        trigger::{CreateSensorInput, SensorRepository},
        Create,
    },
};
use axum::http::StatusCode;
use serde_json::{json, Value};

use helpers::TestContext;

struct SensorLogFixture {
    ctx: TestContext,
    pack: attune_common::models::Pack,
    sensor: attune_common::models::Sensor,
    workload: attune_common::models::OwnedSensorWorkload,
    binding: i64,
    artifact: attune_common::models::Artifact,
}

async fn sensor_log_fixture() -> helpers::Result<SensorLogFixture> {
    use attune_common::repositories::{
        component_lifecycle::PackProjectionIds,
        file_evidence_retention::FileEvidenceRetentionRepository,
        pack::PackRepository,
        runtime::{CreateWorkerInput, WorkerRepository},
        schema_type::SchemaTypeRepository,
        sensor_schema::SensorSchemaRepository,
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadRepository,
        },
    };
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = ctx.user.as_ref().unwrap().id;
    let pack = helpers::create_test_pack(&ctx.pool, "c_sensor_tail").await?;
    PackRepository::set_installed_by(&ctx.pool, pack.id, identity).await?;
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
            r#ref: format!("{}.tail", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Tail receipt consumer".into(),
            description: None,
            entrypoint: "tail".into(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref.clone(),
            runtime_version_constraint: None,
            enabled: true,
            param_schema: Some(json!({"credential":{"type":"string","secret":true}})),
            config: Some(json!({"credential":"c-tail-protected-input"})),
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
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &PackProjectionIds {
            runtimes: vec![runtime.id],
            sensors: vec![sensor.id],
            ..Default::default()
        },
    )
    .await?;
    let worker = WorkerRepository::create(
        &ctx.pool,
        CreateWorkerInput {
            name: "c-sensor-tail-worker".into(),
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
        _ => panic!("sensor tail fixture lease unavailable"),
    };
    let workload = SensorWorkloadRepository::begin_process(&ctx.pool, lease)
        .await?
        .unwrap();
    let mut tx = ctx.pool.begin().await?;
    FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    SensorSchemaRepository::admit(
        &mut tx,
        &workload,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    tx.commit().await?;
    let (binding, _, _) =
        SensorSchemaRepository::find(&ctx.pool, workload.workload_id, workload.generation)
            .await?
            .unwrap();
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
    Ok(SensorLogFixture {
        ctx,
        pack,
        sensor,
        workload,
        binding,
        artifact,
    })
}

#[tokio::test]
async fn sensor_tail_collects_multiple_versions_and_requires_current_decrypt() -> helpers::Result<()>
{
    use attune_common::repositories::{
        artifact::ArtifactVersionRepository,
        file_evidence_retention::FileEvidenceRetentionRepository,
        sensor_schema::SensorSchemaRepository,
    };
    let SensorLogFixture {
        ctx,
        pack,
        sensor,
        binding,
        artifact,
        ..
    } = sensor_log_fixture().await?;
    let identity = ctx.user.as_ref().unwrap().id;
    for content in ["old-one\nold-two\n", "new-three\nnew-four\n"] {
        let mut tx = ctx.pool.begin().await?;
        FileEvidenceRetentionRepository::lock_reclamation(&mut tx).await?;
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
        SensorSchemaRepository::attach_log_version(&mut tx, version.id, binding).await?;
        let path = std::path::Path::new(&ctx.state.config.artifacts_dir)
            .join(version.file_path.as_ref().unwrap());
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        tokio::fs::write(path, content).await?;
        tx.commit().await?;
    }
    for (suffix, actions) in [
        ("visible", json!(["update"])),
        ("plaintext", json!(["decrypt"])),
    ] {
        let permission = PermissionSetRepository::create(
            &ctx.pool,
            CreatePermissionSetInput {
                r#ref: format!("{}.tail_{}", pack.r#ref, suffix),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants: json!([
                    {"resource":"triggers","actions":actions,"constraints":{"refs":[sensor.r#ref]}},
                    {"resource":"packs","actions":["read"],"constraints":{"refs":[pack.r#ref]}}
                ]),
            },
        )
        .await?;
        PermissionAssignmentRepository::create(
            &ctx.pool,
            CreatePermissionAssignmentInput {
                identity,
                permset: permission.id,
            },
        )
        .await?;
        AuthorizationService::invalidate_identity_authz_cache(identity).await;
        AuthorizationService::invalidate_permission_set_caches().await;
        let response = ctx
            .get(
                &format!("/api/v1/sensors/{}/logs/stdout?tail=3", sensor.r#ref),
                None,
            )
            .await?;
        if suffix == "visible" {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert!(!response.text().await?.contains("old-two"));
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.text().await?, "old-two\nnew-three\nnew-four\n");
        }
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn sensor_log_summary_identifies_created_stream_artifacts() {
    let context = TestContext::new()
        .await
        .expect("create test context")
        .with_auth()
        .await
        .expect("authenticate test user");
    let identity = context.user.as_ref().expect("authenticated identity");
    let permission_set = PermissionSetRepository::create(
        &context.pool,
        CreatePermissionSetInput {
            r#ref: format!("sensor_log_test.reader_{}", identity.id),
            pack: None,
            pack_ref: None,
            label: Some("Sensor log reader".to_string()),
            description: None,
            grants: json!([{"resource": "triggers", "actions": ["update"]}]),
        },
    )
    .await
    .expect("create sensor permission set");
    PermissionAssignmentRepository::create(
        &context.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await
    .expect("assign sensor permission set");
    AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    AuthorizationService::invalidate_permission_set_caches().await;

    let runtime = RuntimeRepository::create(
        &context.pool,
        CreateRuntimeInput {
            r#ref: "sensor_log_test.python".to_string(),
            pack: None,
            pack_ref: None,
            description: None,
            name: "python".to_string(),
            aliases: Vec::new(),
            distributions: json!({}),
            installation: None,
            execution_config: json!({"interpreter": {"binary": "python3"}}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await
    .expect("create sensor runtime");
    let sensor_ref = "sensor_log_test.socket_mode";
    SensorRepository::create(
        &context.pool,
        CreateSensorInput {
            r#ref: sensor_ref.to_string(),
            pack: None,
            pack_ref: None,
            label: "Socket Mode".to_string(),
            description: None,
            entrypoint: "socket_mode.py".to_string(),
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
    .await
    .expect("create sensor");
    let stderr = ArtifactRepository::create(
        &context.pool,
        CreateArtifactInput {
            r#ref: format!("sensor.{sensor_ref}.stderr"),
            scope: OwnerType::Sensor,
            owner: sensor_ref.to_string(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 4,
            name: None,
            description: None,
            content_type: Some("text/plain".to_string()),
            data: None,
        },
    )
    .await
    .expect("create stderr artifact");

    let response = context
        .get(&format!("/api/v1/sensors/{sensor_ref}/logs"), None)
        .await
        .expect("request sensor log summary");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("parse sensor log summary");
    assert_eq!(
        body["logs"],
        json!([
            {
                "stream": "stdout",
                "artifact_ref": format!("sensor.{sensor_ref}.stdout"),
                "artifact_id": null
            },
            {
                "stream": "stderr",
                "artifact_ref": format!("sensor.{sensor_ref}.stderr"),
                "artifact_id": stderr.id
            }
        ])
    );
}

#[tokio::test]
async fn staged_sensor_log_source_roundtrip_refresh_expiry_and_release() -> helpers::Result<()> {
    use attune_common::{
        models::LogStreamBackend,
        repositories::{
            artifact::ArtifactVersionRepository,
            file_evidence_retention::FileEvidenceRetentionRepository,
            log_stream::LogStreamRepository, sensor_schema::SensorSchemaRepository,
            sensor_workload::SensorWorkloadRepository,
        },
    };
    let SensorLogFixture {
        ctx,
        pack,
        sensor,
        workload,
        binding,
        artifact,
    } = sensor_log_fixture().await?;
    let version = ArtifactVersionRepository::create_log_pending(
        &ctx.pool,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::ObjectSegments,
        "text/plain".into(),
        None,
        None,
        Some("sensor".into()),
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
    SensorSchemaRepository::attach_log_version(&mut tx, version.id, binding).await?;
    tx.commit().await?;
    let stream = LogStreamRepository::create_with_backend(
        &ctx.pool,
        version.id,
        LogStreamBackend::ObjectSegments,
        1024,
        500,
    )
    .await?;
    let credential = |ttl| -> helpers::Result<attune_api::auth::middleware::AuthenticatedUser> {
        let token = attune_common::auth::jwt::generate_sensor_token_with_cache_authority_and_workload_fence(
            ctx.user.as_ref().unwrap().id, &sensor.r#ref, vec![], Some(&pack.r#ref), &[], &[], workload.fence(), &ctx.state.jwt_config, Some(ttl))?;
        Ok(attune_api::auth::middleware::AuthenticatedUser {
            claims: attune_common::auth::jwt::validate_token(&token, &ctx.state.jwt_config)?,
        })
    };
    let mut historical = None;
    for ttl in [60, 120] {
        let user = credential(ttl)?;
        let tx = ctx.pool.begin().await?;
        let mut source = attune_api::routes::internal_files::log_source::acquire(
            &ctx.state, tx, &user, &version,
        )
        .await
        .map_err(|error| anyhow::anyhow!("sensor source: {error:?}"))?;
        source
            .authorize_target(&user, &version)
            .await
            .map_err(|error| anyhow::anyhow!("sensor target: {error:?}"))?;
        let version =
            ArtifactVersionRepository::find_by_id_for_update(source.transaction(), version.id)
                .await?
                .unwrap();
        let stream = LogStreamRepository::lock(source.transaction(), stream.id).await?;
        let current = source
            .bind(
                &ctx.state,
                &user,
                &version,
                &stream,
                &axum::http::HeaderMap::new(),
                historical.as_ref(),
            )
            .await
            .map_err(|error| anyhow::anyhow!("sensor binding: {error:?}"))?;
        let projected = serde_json::to_value(&current)?;
        assert_eq!(projected["producer"]["kind"], "sensor");
        assert!(projected["producer"]["schema_binding"].is_array());
        assert!(projected["producer"]["log_generation"].is_array());
        historical = Some(current);
        source
            .commit()
            .await
            .map_err(|error| anyhow::anyhow!("sensor commit: {error:?}"))?;
    }
    let mut expired = credential(60)?;
    expired.claims.exp = 0;
    let tx = ctx.pool.begin().await?;
    assert!(attune_api::routes::internal_files::log_source::acquire(
        &ctx.state, tx, &expired, &version
    )
    .await
    .is_err());
    // Actual lifecycle release must progress after both short phase commits.
    assert!(SensorWorkloadRepository::release(&ctx.pool, workload.fence()).await?);
    let user = credential(120)?;
    let tx = ctx.pool.begin().await?;
    assert!(attune_api::routes::internal_files::log_source::acquire(
        &ctx.state, tx, &user, &version
    )
    .await
    .is_err());
    ctx.cleanup().await?;
    Ok(())
}

#[path = "support/object_seal_boundary.rs"]
mod object_seal_boundary;
#[path = "support/object_seal_producers.rs"]
mod object_seal_producers;
