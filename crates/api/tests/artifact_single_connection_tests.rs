//! Selected human reads must never acquire a second connection while retaining
//! their receipt transaction. Exercise the real download route and response body.
mod helpers;
// The shared admitted-pack fixture also contains sensor-only builders.
#[allow(dead_code)]
#[path = "artifact_read_performance/managed_fixture.rs"]
mod managed_fixture;

use std::{sync::Arc, time::Duration};

use attune_common::{
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
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        Create,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{prepare_secret_values, SecretValueInput, ENTITY_EXECUTION_CONFIG},
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use helpers::{Result, TestContext};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn admitted_execution(ctx: &TestContext, action: &str) -> Result<i64> {
    let response = ctx
        .post(
            "/api/v1/executions/execute",
            json!({
                "action_ref": action, "parameters": {}, "permission_set_refs": []
            }),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response.json().await?;
    body["data"]["id"]
        .as_i64()
        .ok_or_else(|| "execution fixture missing".into())
}

async fn origins(ctx: &TestContext, execution: i64, origins: Vec<SecretOrigin>) -> Result<()> {
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/private".into(),
            value: json!("owned fixture"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance {
                origins,
                templates: vec![],
            })?),
        }],
        ctx.state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or("fixture key missing")?,
    )?;
    ExecutionSecretValueRepository::upsert_many(
        &ctx.pool,
        ENTITY_EXECUTION_CONFIG,
        execution,
        &prepared,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn selected_runtime_log_download_completes_with_one_connection_and_recursive_sources(
) -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    let identity = ctx.user.as_ref().ok_or("fixture identity missing")?.id;
    let permission = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "single_connection.reader".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([
                {"resource":"actions","actions":["read","execute"]},
                {"resource":"executions","actions":["read","decrypt"]}
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
    let action = managed_fixture::action_pack(&ctx).await?;
    let source = admitted_execution(&ctx, &action).await?;
    let execution = admitted_execution(&ctx, &action).await?;
    origins(
        &ctx,
        source,
        vec![SecretOrigin::Local {
            source_kind: "parameter_schema".into(),
            source_ref: None,
        }],
    )
    .await?;
    origins(
        &ctx,
        execution,
        vec![SecretOrigin::Entity {
            entity_type: ENTITY_EXECUTION_CONFIG.into(),
            entity_id: source,
            path: "/private".into(),
        }],
    )
    .await?;
    let artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: "artifact_perf.run.single_connection.log".into(),
            scope: OwnerType::Action,
            owner: action,
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 1000,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    let body = b"selected execution output\n";
    let relative = "single-connection/selected.log";
    let root = std::path::Path::new(&ctx.state.config.artifacts_dir);
    std::fs::create_dir_all(root.join("single-connection"))?;
    std::fs::write(root.join(relative), body)?;
    let version = ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact: artifact.id,
            execution: Some(execution),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: Some(relative.into()),
            meta: None,
            created_by: None,
        },
    )
    .await?;

    // This separate, fixture-owned request pool is deliberately constrained;
    // the production path must reuse its one retained transaction, not add pools.
    let options = (*ctx.pool.connect_options())
        .clone()
        .options([("search_path", ctx.schema.as_str())]);
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .connect_with(options)
        .await?;
    let state = Arc::new(
        attune_api::state::AppState::new_with_audit(
            limited.clone(),
            ctx.state.config.as_ref().clone(),
            ctx.state.audit_emitter.clone(),
        )
        .without_authorization_metadata_cache(),
    );
    let app = attune_api::server::Server::new(state).router();
    let path = format!(
        "/api/v1/artifacts/{}/versions/{}/download",
        artifact.id, version.version
    );
    let request = || {
        Request::builder()
            .uri(&path)
            .header("authorization", format!("Bearer {}", ctx.token().unwrap()))
            .body(Body::empty())
            .unwrap()
    };
    let read = |app: axum::Router, request: Request<Body>| async move {
        let response: axum::response::Response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{bytes:?}");
        assert_eq!(bytes.as_ref(), body);
    };
    tokio::time::timeout(Duration::from_secs(15), read(app.clone(), request())).await?;
    let readers = (0..4).map(|_| read(app.clone(), request()));
    tokio::time::timeout(Duration::from_secs(20), futures::future::join_all(readers)).await?;
    assert_eq!(limited.size(), 1);
    drop(app);
    limited.close().await;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn legacy_runtime_and_sensor_downloads_reuse_their_one_retained_connection() -> Result<()> {
    use attune_common::repositories::{
        managed_sensor_metadata::ManagedSensorMetadataRepository,
        sensor_schema::{SensorSchemaRepository, ENTITY_SENSOR_GENERATION_CONFIG},
        trigger::SensorRepository,
        FindByRef,
    };
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    let identity = ctx.user.as_ref().ok_or("fixture identity missing")?.id;
    let permission = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "single_connection.legacy".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource":"actions","actions":["read","execute"]},
            {"resource":"executions","actions":["read","decrypt"]},
            {"resource":"artifacts","actions":["read"],"constraints":{"refs":["single_connection.legacy.log","sensor.artifact_perf.sensor.stdout"]}},
            {"resource":"triggers","actions":["update","decrypt"]}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: permission.id,
        },
    )
    .await?;
    let action = managed_fixture::action_pack(&ctx).await?;
    let execution = admitted_execution(&ctx, &action).await?;
    let token = attune_common::auth::jwt::generate_execution_token_with_permission_sets(
        identity,
        execution,
        &action,
        &ctx.state.jwt_config,
        Some(300),
        std::slice::from_ref(&permission.r#ref),
    )?;
    let runtime = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: "single_connection.legacy.log".into(),
            scope: OwnerType::Action,
            owner: action,
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 1000,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    let root = std::path::Path::new(&ctx.state.config.artifacts_dir);
    std::fs::create_dir_all(root.join("single-connection"))?;
    std::fs::write(
        root.join("single-connection/legacy.log"),
        b"legacy output\n",
    )?;
    let runtime_version = ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact: runtime.id,
            execution: Some(execution),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: Some("single-connection/legacy.log".into()),
            meta: None,
            created_by: None,
        },
    )
    .await?;

    let sensor = SensorRepository::find_by_ref(&ctx.pool, "artifact_perf.sensor")
        .await?
        .ok_or("admitted sensor missing")?;
    let revision = ManagedSensorMetadataRepository::current_revision(&ctx.pool, sensor.id)
        .await?
        .ok_or("admitted sensor revision missing")?;
    let mut tx = ctx.pool.begin().await?;
    let metadata = ManagedSensorMetadataRepository::find(&mut tx, revision).await?;
    let binding = attune_common::sensor_schema::SensorGenerationBinding {
        metadata_revision: Some(revision),
        executable: metadata.executable,
        config: metadata.config_contract,
        triggers: metadata.trigger_contracts,
    };
    let generation = managed_fixture::retained_generation(&mut tx, 1, &binding, json!({})).await?;
    tx.commit().await?;
    let sensor_artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: "sensor.artifact_perf.sensor.stdout".into(),
            scope: OwnerType::Sensor,
            owner: sensor.r#ref,
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 1000,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    std::fs::write(
        root.join("single-connection/sensor.log"),
        b"sensor output\n",
    )?;
    let sensor_version = ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact: sensor_artifact.id,
            execution: None,
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: Some("single-connection/sensor.log".into()),
            meta: None,
            created_by: None,
        },
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    SensorSchemaRepository::attach_log_version(&mut tx, sensor_version.id, generation).await?;
    tx.commit().await?;
    // Force the sensitive sensor branch without inventing a permission proof:
    // its generation and structural owner were produced by real admission above.
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/private".into(),
            value: json!("owned sensor fixture"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: "pack_config".into(),
                    source_ref: None,
                }],
                templates: vec![],
            })?),
        }],
        ctx.state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or("fixture key missing")?,
    )?;
    ExecutionSecretValueRepository::upsert_many(
        &ctx.pool,
        ENTITY_SENSOR_GENERATION_CONFIG,
        generation,
        &prepared,
    )
    .await?;

    let options = (*ctx.pool.connect_options())
        .clone()
        .options([("search_path", ctx.schema.as_str())]);
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .connect_with(options)
        .await?;
    let state = Arc::new(
        attune_api::state::AppState::new_with_audit(
            limited.clone(),
            ctx.state.config.as_ref().clone(),
            ctx.state.audit_emitter.clone(),
        )
        .without_authorization_metadata_cache(),
    );
    let app = attune_api::server::Server::new(state).router();
    let read =
        |app: axum::Router, id: i64, version: i32, version_id: i64, expected: &'static [u8]| {
            let request = Request::builder()
                .uri(format!(
                    "/api/v1/artifacts/{id}/versions/{version}/download?version_id={version_id}"
                ))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            async move {
                let response: axum::response::Response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let body = to_bytes(response.into_body(), 1024).await.unwrap();
                assert_eq!(status, StatusCode::OK, "{body:?}");
                assert_eq!(body.as_ref(), expected);
            }
        };
    tokio::time::timeout(
        Duration::from_secs(15),
        read(
            app.clone(),
            runtime.id,
            runtime_version.version,
            runtime_version.id,
            b"legacy output\n",
        ),
    )
    .await?;
    let concurrent = (0..4).map(|_| {
        read(
            app.clone(),
            runtime.id,
            runtime_version.version,
            runtime_version.id,
            b"legacy output\n",
        )
    });
    tokio::time::timeout(
        Duration::from_secs(20),
        futures::future::join_all(concurrent),
    )
    .await?;
    tokio::time::timeout(
        Duration::from_secs(15),
        read(
            app.clone(),
            sensor_artifact.id,
            sensor_version.version,
            sensor_version.id,
            b"sensor output\n",
        ),
    )
    .await?;
    assert_eq!(limited.size(), 1);
    drop(app);
    limited.close().await;
    ctx.cleanup().await?;
    Ok(())
}
