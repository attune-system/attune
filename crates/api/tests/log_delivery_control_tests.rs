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
        Create, FindById,
    },
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use helpers::{Result, TestContext};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const ROUTE: &str = "/api/v1/internal/log-delivery/control";

#[tokio::test]
async fn control_requires_current_worker_binding_and_exact_custody() -> Result<()> {
    attune_common::auth::install_crypto_provider();
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
    // TestContext::post(None) deliberately uses its stored admin token.
    // A raw request is required to exercise the unauthenticated boundary.
    let unauthenticated = ctx
        .app
        .clone()
        .oneshot(
            Request::post(ROUTE)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request)?))?,
        )
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
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

struct LiveCustody {
    ctx: TestContext,
    worker: i64,
    tokens: WorkerTokenProvider,
    binding: ControlRequest,
    preparation: attune_common::repositories::log_delivery::PrepareLogDelivery,
}

impl LiveCustody {
    async fn new() -> Result<Self> {
        use attune_common::repositories::{
            delegated_execution::DelegatedExecutionRepository, execution::ExecutionRepository,
            runtime::UpdateWorkerInput, Update,
        };
        use sha2::{Digest, Sha256};
        attune_common::auth::install_crypto_provider();
        let ctx = TestContext::new().await?.with_admin_auth().await?;
        let process_token = helpers::delegated_source::token(
            &ctx,
            ctx.user.as_ref().unwrap().id,
            &["standard".into()],
        )
        .await?;
        let claims =
            attune_common::auth::jwt::validate_token(&process_token, &ctx.state.jwt_config)?;
        let execution_id = claims.metadata.as_ref().unwrap()["execution_id"]
            .as_i64()
            .unwrap();
        let execution = ExecutionRepository::find_by_id(&ctx.pool, execution_id)
            .await?
            .unwrap();
        let worker = execution.worker.unwrap();
        let tokens =
            WorkerTokenProvider::new(1, worker.to_string(), (*ctx.state.jwt_config).clone());
        WorkerRepository::update(
            &ctx.pool,
            worker,
            UpdateWorkerInput {
                capabilities: Some(json!({"log_delivery_instance":tokens.instance_id()})),
                ..Default::default()
            },
        )
        .await?;
        let artifact = ArtifactRepository::create(
            &ctx.pool,
            CreateArtifactInput {
                r#ref: format!("{}.stdout.log", execution.action_ref),
                scope: OwnerType::Action,
                owner: execution.action_ref.clone(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 5,
                name: None,
                description: None,
                content_type: None,
                data: None,
            },
        )
        .await?;
        let mut tx = ctx.pool.begin().await?;
        let version = ArtifactVersionRepository::create_log_pending_with_conn(
            &mut tx,
            artifact.id,
            &artifact.r#ref,
            LogStreamBackend::ObjectSegments,
            execution.id,
            json!({"stream":"stdout"}),
        )
        .await?;
        let stream = LogStreamRepository::create_with_backend_with_conn(
            &mut tx,
            version.id,
            LogStreamBackend::ObjectSegments,
            1024,
            100,
        )
        .await?;
        ArtifactVersionRepository::protect_delegated_logs(&mut tx, execution.id, &[version.id])
            .await?;
        let attempt = DelegatedExecutionRepository::current_attempt(&mut tx, execution.id).await?;
        tx.commit().await?;
        let binding = ControlRequest {
            execution: execution.id,
            version: version.id,
            stream: stream.id,
            attempt,
            source_instance: tokens.instance_id(),
            custody: Uuid::new_v4(),
        };
        let digest: String = Sha256::digest(b"x")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let preparation = attune_common::repositories::log_delivery::PrepareLogDelivery {
            execution: binding.execution,
            version: binding.version,
            stream: binding.stream,
            worker,
            instance: binding.source_instance,
            custody: binding.custody,
            segments: vec![attune_common::repositories::log_delivery::DeliverySegment {
                sequence: 0,
                size_bytes: 1,
                sha256: digest.clone(),
            }],
            total_bytes: 1,
            sha256: digest,
            truncated: false,
            authenticated_manifest: None,
            finalize_by: chrono::Utc::now() + chrono::Duration::seconds(60),
        };
        Ok(Self {
            ctx,
            worker,
            tokens,
            binding,
            preparation,
        })
    }
}

#[tokio::test]
async fn delivery_issuance_is_scoped_and_revocation_cannot_be_restored() -> Result<()> {
    use attune_common::{
        models::ExecutionStatus,
        repositories::{
            delegated_execution::DelegatedExecutionRepository,
            execution::ExecutionRepository,
            identity::{IdentityRepository, UpdateIdentityInput},
            log_delivery::LogDeliveryRepository,
            Update,
        },
    };
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let fixture = LiveCustody::new().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    LogDeliveryRepository::prepare(&mut tx, fixture.preparation.clone()).await?;
    ExecutionRepository::transition_worker(
        &mut *tx,
        fixture.binding.execution,
        Some(fixture.worker),
        ExecutionStatus::Completed,
        None,
    )
    .await?;
    DelegatedExecutionRepository::close(&mut tx, fixture.binding.execution).await?;
    tx.commit().await?;
    let worker_token = fixture.tokens.token()?;
    let ControlResponse::Deliver { token } = fixture
        .ctx
        .post(ROUTE, &fixture.binding, Some(&worker_token))
        .await?
        .assert_status(StatusCode::OK)
        .json::<ControlResponse>()
        .await?
    else {
        return Err("delivery credential missing".into());
    };
    let claims = attune_common::auth::jwt::validate_token(&token, &fixture.ctx.state.jwt_config)?;
    let scope =
        attune_common::log_delivery::credential(&claims)?.ok_or("delivery scope missing")?;
    assert_eq!(
        (scope.worker, scope.instance, scope.version),
        (
            fixture.worker,
            fixture.tokens.instance_id(),
            fixture.binding.version
        )
    );
    let owner = fixture.ctx.user.as_ref().unwrap().id;
    IdentityRepository::update(
        &fixture.ctx.pool,
        owner,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        owner,
        UpdateIdentityInput {
            frozen: Some(false),
            ..Default::default()
        },
    )
    .await?;
    let response = fixture
        .ctx
        .post(ROUTE, &fixture.binding, Some(&worker_token))
        .await?
        .assert_status(StatusCode::OK)
        .json::<ControlResponse>()
        .await?;
    assert!(matches!(response, ControlResponse::Settled));
    let request = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/v1/internal/logs/{}/segments/0",
            fixture.binding.version
        ))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(b"x".to_vec()))?;
    let response = fixture.ctx.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let intent = LogDeliveryRepository::find(&fixture.ctx.pool, fixture.binding.version)
        .await?
        .unwrap();
    assert_eq!(intent.state, "revoked");
    fixture.ctx.cleanup().await?;
    Ok(())
}
