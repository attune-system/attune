mod helpers;

use attune_common::repositories::custody_operation::CustodyOperationRepository as Custody;
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
    fn remote_preparation(&self) -> attune_common::log_custody::PrepareCustody {
        attune_common::log_custody::PrepareCustody {
            reservation: attune_common::log_custody::ReserveCustody {
                binding: self.binding.clone(),
                finalize_by: chrono::Utc::now() + chrono::Duration::seconds(60),
            },
            segments: self.preparation.segments.clone(),
            total_bytes: self.preparation.total_bytes,
            sha256: self.preparation.sha256.clone(),
            truncated: self.preparation.truncated,
            authenticated_manifest: self.preparation.authenticated_manifest.clone(),
        }
    }

    async fn new() -> Result<Self> {
        Self::new_with_adhoc(false).await
    }

    async fn new_with_adhoc(is_adhoc: bool) -> Result<Self> {
        use attune_common::repositories::{
            delegated_execution::DelegatedExecutionRepository, execution::ExecutionRepository,
        };
        use sha2::{Digest, Sha256};
        attune_common::auth::install_crypto_provider();
        let ctx = TestContext::new().await?.with_admin_auth().await?;
        let process_token = helpers::delegated_source::token_with_adhoc(
            &ctx,
            ctx.user.as_ref().unwrap().id,
            &["standard".into()],
            is_adhoc,
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
        let prior = WorkerRepository::find_by_id(&ctx.pool, worker)
            .await?
            .unwrap();
        let registered = WorkerRepository::register_incarnation(
            &ctx.pool,
            CreateWorkerInput {
                name: prior.name,
                worker_type: WorkerType::Local,
                runtime: None,
                host: None,
                port: None,
                status: None,
                meta: None,
                capabilities: Some(json!({"log_delivery_instance":tokens.instance_id()})),
            },
            WorkerRole::Action,
            Uuid::new_v4(),
        )
        .await?;
        assert_eq!(registered.id, worker);
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
    delivery_issuance_is_scoped(false).await
}

#[tokio::test]
async fn adhoc_delivery_issuance_is_scoped_and_revocation_cannot_be_restored() -> Result<()> {
    delivery_issuance_is_scoped(true).await
}

async fn delivery_issuance_is_scoped(is_adhoc: bool) -> Result<()> {
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
    let fixture = LiveCustody::new_with_adhoc(is_adhoc).await?;
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

#[tokio::test]
async fn durable_custody_cancel_ack_fences_late_preparation() -> Result<()> {
    cancellation_fences_late_preparation(false).await
}

#[tokio::test]
async fn legacy_settlement_cancels_reserved_preparation() -> Result<()> {
    cancellation_fences_late_preparation(true).await
}

async fn cancellation_fences_late_preparation(legacy: bool) -> Result<()> {
    use attune_common::{
        log_custody::CustodyOperationState, repositories::log_delivery::LogDeliveryRepository,
    };
    let fixture = LiveCustody::new().await?;
    let prepare = fixture.remote_preparation();
    let ack = Custody::reserve(
        &fixture.ctx.pool,
        fixture.worker,
        fixture.tokens.instance_id(),
        &prepare.reservation,
    )
    .await?;
    assert_eq!(ack.state, CustodyOperationState::Reserved);
    let baseline: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_secret_value")
        .fetch_one(&fixture.ctx.pool)
        .await?;
    // The owning task has not entered shared graph admission. No timeout or
    // request cancellation is being used as proof that its effect is absent.
    let (release, blocked) = tokio::sync::oneshot::channel();
    let pool = fixture.ctx.pool.clone();
    let worker = fixture.worker;
    let instance = fixture.tokens.instance_id();
    let delayed_request = prepare.clone();
    let mut delayed = tokio::spawn(async move {
        blocked.await.expect("preparation gate");
        Custody::prepare(&pool, worker, instance, &delayed_request).await
    });
    let mut joined = false;
    let exchange = async {
        if legacy {
            let token = fixture.tokens.token()?;
            for changed in [
                ControlRequest {
                    attempt: Uuid::new_v4(),
                    ..fixture.binding.clone()
                },
                ControlRequest {
                    stream: fixture.binding.stream + 1,
                    ..fixture.binding.clone()
                },
                ControlRequest {
                    execution: fixture.binding.execution + 1,
                    ..fixture.binding.clone()
                },
            ] {
                fixture
                    .ctx
                    .post(ROUTE, &changed, Some(&token))
                    .await?
                    .assert_status(StatusCode::FORBIDDEN);
            }
            let cross_worker = generate_worker_token_with_instance(
                1,
                &(worker + 1).to_string(),
                instance,
                &fixture.ctx.state.jwt_config,
                Some(60),
            )?;
            fixture
                .ctx
                .post(ROUTE, &fixture.binding, Some(&cross_worker))
                .await?
                .assert_status(StatusCode::FORBIDDEN);
            let missing = ControlRequest {
                custody: Uuid::new_v4(),
                ..fixture.binding.clone()
            };
            let response = fixture
                .ctx
                .post(ROUTE, &missing, Some(&token))
                .await?
                .assert_status(StatusCode::OK)
                .json::<ControlResponse>()
                .await?;
            assert!(matches!(response, ControlResponse::Waiting));
            let response = fixture
                .ctx
                .post(ROUTE, &fixture.binding, Some(&token))
                .await?
                .assert_status(StatusCode::OK)
                .json::<ControlResponse>()
                .await?;
            assert!(matches!(response, ControlResponse::Settled));
        } else {
            assert!(
                Custody::cancel_absent(&fixture.ctx.pool, worker, instance, &prepare.reservation)
                    .await?
                    .absent
            );
        }
        release.send(()).map_err(|_| "preparation disappeared")?;
        let response = (&mut delayed).await;
        joined = true;
        assert!(matches!(
            response?,
            Err(attune_common::Error::PermissionDenied(_))
        ));
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    if !joined {
        delayed.abort();
        let _ = (&mut delayed).await;
    }
    if let Err(error) = exchange {
        fixture.ctx.cleanup().await?;
        return Err(error);
    }
    // The ordinary local preparation path shares the same permanent tombstone.
    let mut local = fixture.preparation.clone();
    local.finalize_by = prepare.reservation.finalize_by;
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(LogDeliveryRepository::prepare(&mut tx, local)
        .await
        .is_err());
    tx.rollback().await?;
    let (state,intents,secrets):(String,i64,i64)=sqlx::query_as("SELECT state,(SELECT COUNT(*) FROM log_delivery_intent),(SELECT COUNT(*) FROM execution_secret_value) FROM log_custody_operation WHERE custody=$1")
        .bind(fixture.binding.custody).fetch_one(&fixture.ctx.pool).await?;
    assert_eq!(state, "cancelled");
    assert_eq!(intents, 0);
    assert_eq!(secrets, baseline);
    use attune_common::repositories::{
        identity::{IdentityRepository, UpdateIdentityInput},
        Update,
    };
    for frozen in [true, false] {
        IdentityRepository::update(
            &fixture.ctx.pool,
            fixture.ctx.user.as_ref().unwrap().id,
            UpdateIdentityInput {
                frozen: Some(frozen),
                ..Default::default()
            },
        )
        .await?;
    }
    // A fresh caller after source restoration still sees the retained fence.
    assert!(
        Custody::prepare(&fixture.ctx.pool, worker, instance, &prepare)
            .await
            .is_err()
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn durable_custody_reservation_does_not_revive_after_owner_restore() -> Result<()> {
    let fixture = LiveCustody::new().await?;
    let prepare = fixture.remote_preparation();
    let pool = &fixture.ctx.pool;
    let instance = fixture.tokens.instance_id();
    Custody::reserve(pool, fixture.worker, instance, &prepare.reservation).await?;
    for frozen in [true, false] {
        sqlx::query("UPDATE identity SET frozen=$2 WHERE id=$1")
            .bind(fixture.ctx.user.as_ref().unwrap().id)
            .bind(frozen)
            .execute(pool)
            .await?;
    }
    assert!(
        Custody::reserve(pool, fixture.worker, instance, &prepare.reservation)
            .await
            .is_err()
    );
    assert!(Custody::prepare(pool, fixture.worker, instance, &prepare)
        .await
        .is_err());
    let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_delivery_intent")
        .fetch_one(pool)
        .await?;
    assert_eq!(intents, 0);
    // A new independently bound reservation after restoration can succeed.
    let old = ArtifactVersionRepository::find_by_id(pool, fixture.binding.version)
        .await?
        .unwrap();
    let artifact = ArtifactRepository::find_by_id(pool, old.artifact)
        .await?
        .unwrap();
    let mut tx = pool.begin().await?;
    let version = ArtifactVersionRepository::create_log_pending_with_conn(
        &mut tx,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::ObjectSegments,
        fixture.binding.execution,
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
    ArtifactVersionRepository::protect_delegated_logs(
        &mut tx,
        fixture.binding.execution,
        &[version.id],
    )
    .await?;
    tx.commit().await?;
    let mut new = prepare.reservation.clone();
    new.binding.version = version.id;
    new.binding.stream = stream.id;
    new.binding.custody = Uuid::new_v4();
    Custody::reserve(pool, fixture.worker, instance, &new).await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn custody_waiter(pool: &sqlx::PgPool, blocker: i32) -> Result<i32> {
    Ok(tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            let waiter:Option<i32>=sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND pid<>pg_backend_pid() ORDER BY pid LIMIT 1")
                .bind(blocker).fetch_optional(pool).await?;
            if let Some(waiter)=waiter {return Ok::<_,sqlx::Error>(waiter);}
            tokio::task::yield_now().await;
        }
    }).await??)
}

#[tokio::test]
async fn durable_custody_absence_waits_for_admitted_preparation_commit() -> Result<()> {
    cancellation_waits_for_admitted_preparation(false).await
}

#[tokio::test]
async fn legacy_settlement_waits_for_admitted_preparation_commit() -> Result<()> {
    cancellation_waits_for_admitted_preparation(true).await
}

async fn cancellation_waits_for_admitted_preparation(legacy: bool) -> Result<()> {
    let fixture = LiveCustody::new().await?;
    let prepare = fixture.remote_preparation();
    let pool = &fixture.ctx.pool;
    let worker = fixture.worker;
    let instance = fixture.tokens.instance_id();
    Custody::reserve(pool, worker, instance, &prepare.reservation).await?;
    let mut barrier = pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await?;
    let _: i64 = sqlx::query_scalar("SELECT id FROM artifact_version WHERE id=$1 FOR UPDATE")
        .bind(fixture.binding.version)
        .fetch_one(&mut *barrier)
        .await?;
    let mut barrier = Some(barrier);
    let mut tasks = tokio::task::JoinSet::new();
    let owned_pool = pool.clone();
    let request = prepare.clone();
    tasks.spawn(async move {
        Custody::prepare(&owned_pool, worker, instance, &request).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((false, false))
    });
    let exchange = async {
        let preparing = custody_waiter(pool, blocker).await?;
        let owned_pool = pool.clone();
        let reservation = prepare.reservation.clone();
        let app = fixture.ctx.app.clone();
        let token = fixture.tokens.token()?;
        let binding = fixture.binding.clone();
        tasks.spawn(async move {
            let absent = if legacy {
                let request = Request::post(ROUTE)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(serde_json::to_vec(&binding)?))?;
                let response = app.oneshot(request).await?;
                assert_eq!(response.status(), StatusCode::OK);
                let body = axum::body::to_bytes(response.into_body(), 4096).await?;
                match serde_json::from_slice::<ControlResponse>(&body)? {
                    ControlResponse::Waiting => false,
                    _ => return Err("control settled or delegated pending preparation".into()),
                }
            } else {
                Custody::cancel_absent(&owned_pool, worker, instance, &reservation)
                    .await?
                    .absent
            };
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((true, absent))
        });
        custody_waiter(pool, preparing).await?;
        barrier
            .take()
            .ok_or("source barrier lost")?
            .commit()
            .await?;
        let mut prepared = false;
        let mut absence = None;
        while let Some(result) = tasks.join_next().await {
            let (cancel, absent) = result?.map_err(|e| e as Box<dyn std::error::Error>)?;
            if cancel {
                absence = Some(absent);
            } else {
                prepared = true;
            }
        }
        assert!(prepared);
        assert_eq!(absence, Some(false));
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    if let Some(barrier) = barrier.take() {
        barrier.rollback().await?;
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if let Err(error) = exchange {
        fixture.ctx.cleanup().await?;
        return Err(error);
    }
    let (state,intents):(String,i64)=sqlx::query_as("SELECT state,(SELECT COUNT(*) FROM log_delivery_intent WHERE custody=$1) FROM log_custody_operation WHERE custody=$1")
        .bind(fixture.binding.custody).fetch_one(pool).await?;
    assert_eq!(state, "prepared");
    assert_eq!(intents, 1);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn custody_handover_can_cancel_retained_old_reservation_but_never_prepare_it() -> Result<()> {
    let fixture = LiveCustody::new().await?;
    let request = fixture.remote_preparation();
    let pool = &fixture.ctx.pool;
    let worker = fixture.worker;
    let first = fixture.tokens.instance_id();
    Custody::reserve(pool, worker, first, &request.reservation).await?;
    let prior = WorkerRepository::find_by_id(pool, worker).await?.unwrap();
    let second = Uuid::new_v4();
    WorkerRepository::register_incarnation(
        pool,
        CreateWorkerInput {
            name: prior.name,
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: None,
            meta: None,
            capabilities: Some(json!({"log_delivery_instance":second})),
        },
        WorkerRole::Action,
        Uuid::new_v4(),
    )
    .await?;
    for instance in [first, second] {
        assert!(Custody::prepare(pool, worker, instance, &request)
            .await
            .is_err());
    }
    assert!(
        Custody::cancel_absent(pool, worker, second, &request.reservation)
            .await?
            .absent
    );
    let (state,intents):(String,i64)=sqlx::query_as("SELECT state,(SELECT COUNT(*) FROM log_delivery_intent WHERE custody=$1) FROM log_custody_operation WHERE custody=$1")
        .bind(request.reservation.binding.custody).fetch_one(pool).await?;
    assert_eq!(state, "cancelled");
    assert_eq!(intents, 0);
    let mut unreserved = request.reservation.clone();
    unreserved.binding.custody = Uuid::new_v4();
    assert!(Custody::cancel_absent(pool, worker, second, &unreserved)
        .await
        .is_err());
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn trusted_custody_orphan_requires_exact_retained_target_and_committed_tombstone(
) -> Result<()> {
    let fixture = LiveCustody::new().await?;
    let mut prepare = fixture.remote_preparation();
    // Cancellation owns settlement, not upload authority, and must work after
    // expiry. Missing proof in the genuine HTTP route still returns Waiting.
    prepare.reservation.finalize_by = chrono::Utc::now() - chrono::Duration::seconds(1);
    for binding in [
        ControlRequest {
            execution: fixture.binding.execution + 1,
            ..fixture.binding.clone()
        },
        ControlRequest {
            attempt: Uuid::new_v4(),
            ..fixture.binding.clone()
        },
        ControlRequest {
            stream: fixture.binding.stream + 1,
            ..fixture.binding.clone()
        },
    ] {
        let invalid = attune_common::log_custody::ReserveCustody {
            binding,
            finalize_by: prepare.reservation.finalize_by,
        };
        assert!(Custody::cancel_absent(
            &fixture.ctx.pool,
            fixture.worker,
            fixture.tokens.instance_id(),
            &invalid
        )
        .await
        .is_err());
    }
    let token = fixture.tokens.token()?;
    let response = fixture
        .ctx
        .post(ROUTE, &fixture.binding, Some(&token))
        .await?
        .assert_status(StatusCode::OK)
        .json::<ControlResponse>()
        .await?;
    assert!(matches!(response, ControlResponse::Waiting));
    assert!(
        Custody::cancel_absent(
            &fixture.ctx.pool,
            fixture.worker,
            fixture.tokens.instance_id(),
            &prepare.reservation
        )
        .await?
        .absent
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM log_custody_operation WHERE custody=$1")
            .bind(fixture.binding.custody)
            .fetch_one(&fixture.ctx.pool)
            .await?;
    assert_eq!(state, "cancelled");
    // Neither an extended deadline nor a fresh invocation can resurrect it.
    prepare.reservation.finalize_by = chrono::Utc::now() + chrono::Duration::seconds(60);
    assert!(Custody::reserve(
        &fixture.ctx.pool,
        fixture.worker,
        fixture.tokens.instance_id(),
        &prepare.reservation
    )
    .await
    .is_err());
    assert!(Custody::prepare(
        &fixture.ctx.pool,
        fixture.worker,
        fixture.tokens.instance_id(),
        &prepare
    )
    .await
    .is_err());
    fixture.ctx.cleanup().await?;
    Ok(())
}
