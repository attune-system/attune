mod helpers;

use attune_common::{
    auth::{
        jwt::{generate_worker_token, generate_worker_token_with_instance},
        WorkerTokenProvider,
    },
    log_delivery::{ControlRequest, ControlResponse},
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, LogStreamBackend, OwnerType,
        RetentionPolicyType, WorkerRole, WorkerType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        log_stream::LogStreamRepository,
        runtime::{CreateWorkerInput, WorkerRepository},
        Create,
    },
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::json;
use uuid::Uuid;

const ROUTE: &str = "/api/v1/internal/log-delivery/control";

#[tokio::test]
async fn control_requires_current_worker_binding_and_exact_custody() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let tokens = WorkerTokenProvider::new(1, "unregistered", (*ctx.state.jwt_config).clone());
    let incarnation = Uuid::new_v4();
    let worker = WorkerRepository::register_incarnation(
        &ctx.pool,
        CreateWorkerInput {
            name: format!("control-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: None,
            meta: None,
            capabilities: Some(json!({"log_delivery_instance": tokens.instance_id()})),
        },
        WorkerRole::Action,
        incarnation,
    )
    .await?;
    tokens.set_worker_id(worker.id.to_string());
    let artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: "test.custody-control".into(),
            scope: OwnerType::System,
            owner: "".into(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 1,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await?;
    let version = ArtifactVersionRepository::create_log_pending(
        &ctx.pool,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::ObjectSegments,
        "text/plain".into(),
        Some(1),
        None,
        None,
    )
    .await?;
    let stream = LogStreamRepository::create_with_backend(
        &ctx.pool,
        version.id,
        LogStreamBackend::ObjectSegments,
        1024,
        500,
    )
    .await?;
    let request = ControlRequest {
        execution: 1,
        version: version.id,
        stream: stream.id,
        attempt: Uuid::new_v4(),
        source_instance: tokens.instance_id(),
        custody: Uuid::new_v4(),
    };
    // Synthetic prepared custody deliberately has no live execution authority.
    // It may wait/expire, but must never receive an upload credential.
    sqlx::query("INSERT INTO log_delivery_intent(execution,artifact_version,stream,worker,source_instance,delivery_instance,attempt,owner,custody,segments,total_bytes,sha256,truncated,finalize_by) VALUES($1,$2,$3,$4,$5,$5,$6,1,$7,$8,1,$9,false,clock_timestamp()+interval '1 minute')")
        .bind(request.execution).bind(request.version).bind(request.stream).bind(worker.id)
        .bind(request.source_instance).bind(request.attempt).bind(request.custody)
        .bind(json!([{"sequence":0,"size_bytes":1,"sha256":"0".repeat(64)}]))
        .bind("0".repeat(64)).execute(&ctx.pool).await?;
    ctx.post(ROUTE, &request, None)
        .await?
        .assert_status(StatusCode::UNAUTHORIZED);
    ctx.post(ROUTE, &request, ctx.token())
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    let token = tokens.token()?;
    let response = ctx
        .post(ROUTE, &request, Some(&token))
        .await?
        .assert_status(StatusCode::OK)
        .json::<ControlResponse>()
        .await?;
    assert!(matches!(response, ControlResponse::Waiting));
    for changed in [
        ControlRequest {
            attempt: Uuid::new_v4(),
            ..request.clone()
        },
        ControlRequest {
            custody: Uuid::new_v4(),
            ..request.clone()
        },
        ControlRequest {
            source_instance: Uuid::new_v4(),
            ..request.clone()
        },
        ControlRequest {
            stream: request.stream + 1,
            ..request.clone()
        },
        ControlRequest {
            execution: request.execution + 1,
            ..request.clone()
        },
    ] {
        ctx.post(ROUTE, &changed, Some(&token))
            .await?
            .assert_status(StatusCode::FORBIDDEN);
    }
    let stale = generate_worker_token_with_instance(
        1,
        &worker.id.to_string(),
        Uuid::new_v4(),
        &ctx.state.jwt_config,
        Some(60),
    )?;
    ctx.post(ROUTE, &request, Some(&stale))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    let cross_worker = generate_worker_token_with_instance(
        1,
        &(worker.id + 1).to_string(),
        tokens.instance_id(),
        &ctx.state.jwt_config,
        Some(60),
    )?;
    ctx.post(ROUTE, &request, Some(&cross_worker))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    let unfenced =
        generate_worker_token(1, &worker.id.to_string(), &ctx.state.jwt_config, Some(60))?;
    ctx.post(ROUTE, &request, Some(&unfenced))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    WorkerRepository::deregister_incarnation(&ctx.pool, worker.id, incarnation).await?;
    ctx.post(ROUTE, &request, Some(&token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    ctx.cleanup().await?;
    Ok(())
}
