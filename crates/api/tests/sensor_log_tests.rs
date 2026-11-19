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
    sensor_log_fixture_with_native_metadata(false).await
}

async fn sensor_log_fixture_with_native_metadata(
    native: bool,
) -> helpers::Result<SensorLogFixture> {
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
    if native {
        use attune_common::repositories::{
            component_configuration::ComponentConfigurationRepository,
            managed_sensor_metadata::ManagedSensorMetadataRepository, FindById,
        };
        let mut tx = ctx.pool.begin().await?;
        ManagedSensorMetadataRepository::lock_mutation(&mut tx).await?;
        let current = SensorRepository::find_by_id(&mut *tx, sensor.id)
            .await?
            .unwrap();
        let key = ctx.state.config.security.encryption_key.as_deref();
        ComponentConfigurationRepository::bind_sensor(
            &mut tx,
            &current,
            current.config.as_ref(),
            key,
        )
        .await?;
        ManagedSensorMetadataRepository::readmit(&mut tx, &current, identity, key).await?;
        tx.commit().await?;
    }
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

/// Real internal-file HTTP handlers and an owned filesystem object store; provider
/// stopped facts are repository fixture inputs, not Kubernetes/Docker OS proof.
#[tokio::test]
async fn native_sensor_ordinary_files_publish_after_stop_without_run_or_legacy_fallback(
) -> helpers::Result<()> {
    use attune_common::{
        artifact_transport::DirectUploadResponse,
        auth::jwt::{
            generate_sensor_token_with_cache_authority_and_workload_fence,
            generate_worker_token_with_instance,
        },
        isolated_process::ResourceIdentity,
        repositories::{
            artifact::ArtifactVersionRepository, runtime::WorkerRepository,
            sensor_workload::SensorWorkloadRepository,
        },
        workload_launch::{LaunchExitOutcome, Sha256Digest},
    };
    use axum::{
        body::Body,
        http::{header, Method, Request},
    };
    use tower::ServiceExt;
    let SensorLogFixture {
        ctx,
        pack,
        sensor,
        workload,
        ..
    } = sensor_log_fixture_with_native_metadata(true).await?;
    WorkerRepository::update_heartbeat(&ctx.pool, workload.worker_id).await?;
    let artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: format!("sensor.{}.ordinary", sensor.r#ref),
            scope: OwnerType::Sensor,
            owner: sensor.r#ref.clone(),
            r#type: ArtifactType::FileBinary,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 10,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await?;
    let historical = ArtifactVersionRepository::create_file_backed(
        &ctx.pool,
        artifact.id,
        &artifact.r#ref,
        "application/octet-stream".into(),
        None,
        None,
        None,
    )
    .await?;
    let identity = ctx.user.as_ref().unwrap().id;
    let manager = generate_worker_token_with_instance(
        identity,
        &workload.worker_id.to_string(),
        workload.worker_instance,
        &ctx.state.jwt_config,
        Some(300),
    )?;
    let replacement = generate_worker_token_with_instance(
        identity,
        &workload.worker_id.to_string(),
        uuid::Uuid::new_v4(),
        &ctx.state.jwt_config,
        Some(300),
    )?;
    let sensor_token = generate_sensor_token_with_cache_authority_and_workload_fence(
        identity,
        &sensor.r#ref,
        vec![],
        Some(&pack.r#ref),
        &[],
        &[],
        workload.fence(),
        &ctx.state.jwt_config,
        Some(300),
    )?;
    let intent = uuid::Uuid::new_v4();
    SensorWorkloadRepository::reserve_native_effect(
        &ctx.pool,
        sensor.id,
        workload.fence(),
        intent,
        "private HTTP fixture",
        ctx.state.config.security.encryption_key.as_deref().unwrap(),
    )
    .await?;
    SensorWorkloadRepository::consume_native_effect(
        &ctx.pool,
        sensor.id,
        workload.fence(),
        intent,
        false,
    )
    .await?;
    let resource = ResourceIdentity::Kubernetes {
        cluster: "owned-fixture".into(),
        namespace: "owned-fixture".into(),
        pod_name: "owned-fixture".into(),
        pod_uid: uuid::Uuid::new_v4().to_string(),
    };
    SensorWorkloadRepository::record_native_resource(
        &ctx.pool,
        workload.workload_id,
        intent,
        &resource,
    )
    .await?;
    SensorWorkloadRepository::consume_native_effect(
        &ctx.pool,
        sensor.id,
        workload.fence(),
        intent,
        true,
    )
    .await?;
    let mut selected = Vec::new();
    for _ in 0..3 {
        let response = ctx
            .post(
                &format!("/api/v1/artifacts/{}/versions/file", artifact.id),
                json!({"content_type":"application/octet-stream"}),
                Some(&sensor_token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value = response.json().await?;
        let id = body["data"]["id"].as_i64().unwrap();
        let path = body["data"]["file_path"].as_str().unwrap().to_string();
        assert!(
            ArtifactVersionRepository::sensor_native_file_binding(&ctx.pool, id)
                .await?
                .is_some()
        );
        selected.push((id, path));
    }
    SensorWorkloadRepository::stop_native_effect(&ctx.pool, workload.workload_id, intent).await?;
    SensorWorkloadRepository::record_native_exit(
        &ctx.pool,
        workload.workload_id,
        intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await?;
    SensorWorkloadRepository::release(&ctx.pool, workload.fence()).await?;
    assert!(!SensorWorkloadRepository::is_current_fence(&ctx.pool, workload.fence()).await?);

    async fn put(
        ctx: &TestContext,
        path: &str,
        token: &str,
        bytes: &[u8],
        digest: bool,
    ) -> helpers::Result<axum::response::Response> {
        let mut request = Request::builder()
            .method(Method::PUT)
            .uri(format!("/api/v1/internal/files/{path}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, bytes.len().to_string());
        if digest {
            request = request.header("x-attune-sha256", Sha256Digest::of_bytes(bytes).as_str());
        }
        Ok(ctx
            .app
            .clone()
            .oneshot(request.body(Body::from(bytes.to_vec()))?)
            .await?)
    }
    // Neither a replacement manager nor the closed Sensor Run can publish.
    assert_eq!(
        put(&ctx, &selected[0].1, &replacement, b"denied", true)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        put(&ctx, &selected[0].1, &sensor_token, b"denied", true)
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    // Missing binding must not fall through to a current manager's generic
    // owner/ref authorization, including the no-digest plaintext staging path.
    assert_eq!(
        put(
            &ctx,
            historical.file_path.as_deref().unwrap(),
            &manager,
            b"denied plaintext",
            false
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert!(!std::path::Path::new(&ctx.state.config.artifacts_dir)
        .join(historical.file_path.as_ref().unwrap())
        .exists());
    for ((id, path), bytes) in selected
        .iter()
        .take(2)
        .zip([b"ordinary native file".as_slice(), b"".as_slice()])
    {
        let response = put(&ctx, path, &manager, bytes, true).await?;
        assert_eq!(response.status(), StatusCode::CREATED);
        let ready = ArtifactVersionRepository::find_by_id(&ctx.pool, *id)
            .await?
            .unwrap();
        assert_eq!(
            ready.body_state,
            Some(attune_common::models::enums::ArtifactBodyState::Ready)
        );
        let receipt = ctx
            .post(
                &format!("/api/v1/internal/artifacts/complete/{path}"),
                json!({}),
                Some(&manager),
            )
            .await?;
        assert_eq!(receipt.status(), StatusCode::OK);
        assert_eq!(
            receipt.headers().get("x-attune-size").unwrap().to_str()?,
            bytes.len().to_string()
        );
        SensorWorkloadRepository::record_native_file_receipt(
            &ctx.pool,
            workload.workload_id,
            intent,
            *id,
            bytes.len() as u64,
        )
        .await?;
        // The API Ready branch needs no guest/local staging or plaintext PUT.
        assert!(!std::path::Path::new(&ctx.state.config.artifacts_dir)
            .join(path)
            .exists());
        let direct=ctx.post(&format!("/api/v1/internal/artifacts/direct-upload/{path}"),json!({"size_bytes":bytes.len(),"sha256":Sha256Digest::of_bytes(bytes).as_str(),"content_type":"application/octet-stream"}),Some(&manager)).await?;
        assert_eq!(direct.status(), StatusCode::OK);
        assert!(
            matches!(direct.json::<DirectUploadResponse>().await?,DirectUploadResponse::AlreadyReady {size_bytes} if size_bytes==bytes.len() as u64)
        );
        assert_eq!(
            put(&ctx, path, &manager, b"late plaintext", false)
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    SensorWorkloadRepository::record_native_capture(&ctx.pool, workload.workload_id, intent, false)
        .await?;
    SensorWorkloadRepository::record_native_capture(&ctx.pool, workload.workload_id, intent, true)
        .await?;
    assert!(
        !SensorWorkloadRepository::native_files_complete(&ctx.pool, workload.workload_id, intent)
            .await?,
        "the remaining real Pending obligation blocks settlement"
    );
    // Fault injection: an orphaned old binding never becomes a legacy write
    // grant merely because its ledger row was cleared. No provider cleanup is
    // claimed or performed by this HTTP authorization fixture.
    sqlx::query("UPDATE sensor_workload_assignment SET native_effect=NULL WHERE workload=$1")
        .bind(workload.workload_id)
        .execute(&ctx.pool)
        .await?;
    assert!(
        !SensorWorkloadRepository::is_current_fence(&ctx.pool, workload.fence()).await?,
        "clearing missing native evidence cannot revive the closed generation"
    );
    assert_eq!(
        put(
            &ctx,
            &selected[2].1,
            &manager,
            b"orphaned old intent",
            false
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        put(
            &ctx,
            historical.file_path.as_deref().unwrap(),
            &manager,
            b"old unbound",
            false
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    ctx.cleanup().await?;
    Ok(())
}
