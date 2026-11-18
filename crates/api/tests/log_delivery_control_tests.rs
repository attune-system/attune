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
    async fn enroll_host(&self) -> Result<attune_common::workload_host::RegisteredWorkloadHost> {
        use attune_common::{
            repositories::workload_host::WorkloadHostRepository,
            workload_host::{EnrollmentActor, RegisterWorkloadHost},
        };
        // A distinct manager identity remains available to prove negative
        // settlement after the action owner's authority has been revoked.
        let token = self
            .ctx
            .create_test_user(&format!("custodian-{}", Uuid::new_v4()))
            .await?;
        let identity =
            attune_common::auth::jwt::validate_token(&token, &self.ctx.state.jwt_config)?
                .sub
                .parse::<i64>()?;
        let engine:i64=sqlx::query_scalar("INSERT INTO workload_engine(pool,controller_url,daemon_id,platform) VALUES($1,'https://owned-controller.invalid','owned-daemon','linux/amd64') RETURNING id")
            .bind(format!("custody-{}",Uuid::new_v4())).fetch_one(&self.ctx.pool).await?;
        let digest = attune_common::workload_launch::Sha256Digest::of_bytes(b"owned-custody-image");
        let image:i64=sqlx::query_scalar("INSERT INTO workload_execution_image(engine,reference,sha256,platform) VALUES($1,$2,$3,'linux/amd64') RETURNING id")
            .bind(engine).bind(format!("sha256:{}",digest.as_str())).bind(digest.as_str()).fetch_one(&self.ctx.pool).await?;
        let profile:i64=sqlx::query_scalar("INSERT INTO workload_host_profile(image,role,capabilities,max_concurrent) VALUES($1,'action','{}'::jsonb,1) RETURNING id")
            .bind(image).fetch_one(&self.ctx.pool).await?;
        let name: String = sqlx::query_scalar("SELECT name FROM worker WHERE id=$1")
            .bind(self.worker)
            .fetch_one(&self.ctx.pool)
            .await?;
        // Adopt the fixture's already-assigned logical slot with a complete
        // prior registration binding. Normal register performs the handover,
        // advances its generation and installs the current incarnation/heartbeat.
        let enrollment:i64=sqlx::query_scalar("INSERT INTO workload_host_enrollment(profile,identity,worker_name,worker,instance,custody_instance,expires_at) VALUES($1,$2,$3,$4,$5,$6,clock_timestamp()+interval '1 hour') RETURNING id")
            .bind(profile).bind(identity).bind(name).bind(self.worker)
            .bind(Uuid::new_v4()).bind(self.binding.source_instance)
            .fetch_one(&self.ctx.pool).await?;
        Ok(WorkloadHostRepository::register(
            &self.ctx.pool,
            EnrollmentActor {
                identity,
                enrollment,
                profile,
                generation: 0,
                expires_at: chrono::Utc::now().timestamp() + 1800,
            },
            &RegisterWorkloadHost {
                instance: Uuid::new_v4(),
                custody_instance: self.binding.source_instance,
            },
            &self.ctx.state.jwt_config,
        )
        .await?)
    }

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

#[tokio::test]
async fn durable_custody_cancel_ack_fences_late_remote_and_local_preparation() -> Result<()> {
    cancellation_fences_late_preparation(false).await
}

#[tokio::test]
async fn legacy_settlement_cancels_reserved_remote_and_local_preparation() -> Result<()> {
    cancellation_fences_late_preparation(true).await
}

async fn cancellation_fences_late_preparation(legacy: bool) -> Result<()> {
    use attune_common::{
        log_custody::{CustodyAbsence, CustodyOperationAck, CustodyOperationState},
        repositories::log_delivery::LogDeliveryRepository,
    };
    let fixture = LiveCustody::new().await?;
    let host = fixture.enroll_host().await?;
    let prepare = fixture.remote_preparation();
    let reserve = "/api/v1/workload-hosts/custody/reserve";
    let cancel = "/api/v1/workload-hosts/custody/cancel-absent";
    fixture
        .ctx
        .post(reserve, &prepare.reservation, fixture.ctx.token())
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    let ack = fixture
        .ctx
        .post(
            reserve,
            &prepare.reservation,
            Some(host.session.token.as_str()),
        )
        .await?
        .assert_status(StatusCode::OK)
        .json::<CustodyOperationAck>()
        .await?;
    assert_eq!(ack.state, CustodyOperationState::Reserved);
    let baseline: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_secret_value")
        .fetch_one(&fixture.ctx.pool)
        .await?;
    // Reserve is committed/ACKed. This actual HTTP consumer cannot enter graph
    // admission until the cancellation's committed absence ACK is observed.
    let (release, blocked) = tokio::sync::oneshot::channel();
    let app = fixture.ctx.app.clone();
    let request = Request::post("/api/v1/workload-hosts/custody/prepare")
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("Bearer {}", host.session.token.as_str()),
        )
        .body(Body::from(serde_json::to_vec(&prepare)?))?;
    let mut delayed = tokio::spawn(async move {
        blocked
            .await
            .map_err(|_| "delayed preparation gate closed")?;
        app.oneshot(request)
            .await
            .map_err(|_| "delayed preparation transport failed")
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
                &(fixture.worker + 1).to_string(),
                fixture.tokens.instance_id(),
                &fixture.ctx.state.jwt_config,
                Some(60),
            )?;
            fixture
                .ctx
                .post(ROUTE, &fixture.binding, Some(&cross_worker))
                .await?
                .assert_status(StatusCode::FORBIDDEN);
            // Missing retained operation evidence cannot acknowledge absence,
            // even with a valid worker credential and otherwise exact target.
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
            let cancellation = fixture
                .ctx
                .post(
                    cancel,
                    &prepare.reservation,
                    Some(host.session.token.as_str()),
                )
                .await?;
            if cancellation.status() != StatusCode::OK {
                return Err("custody cancellation did not ACK".into());
            }
            let ack = cancellation.json::<CustodyAbsence>().await?;
            if !ack.absent {
                return Err("custody cancellation did not prove absence".into());
            }
        }
        release
            .send(())
            .map_err(|_| "delayed preparation disappeared")?;
        let response = (&mut delayed).await;
        joined = true;
        if response??.status() != StatusCode::FORBIDDEN {
            return Err("late preparation was admitted".into());
        }
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
    // The transitional in-process writer is gated by the same tombstone; it
    // cannot provide a raw preparation bypass for the delayed effect.
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
    // Restoring source authority cannot restore a cancelled operation.
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
    // A new HTTP request/router consumer sees the durable fence too.
    fixture
        .ctx
        .post(
            "/api/v1/workload-hosts/custody/prepare",
            &prepare,
            Some(host.session.token.as_str()),
        )
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn durable_custody_reservation_does_not_revive_after_owner_restore() -> Result<()> {
    use attune_common::log_custody::CustodyOperationAck;
    let fixture = LiveCustody::new().await?;
    let host = fixture.enroll_host().await?;
    let prepare = fixture.remote_preparation();
    fixture
        .ctx
        .post(
            "/api/v1/workload-hosts/custody/reserve",
            &prepare.reservation,
            Some(host.session.token.as_str()),
        )
        .await?
        .assert_status(StatusCode::OK)
        .json::<CustodyOperationAck>()
        .await?;
    for frozen in [true, false] {
        sqlx::query("UPDATE identity SET frozen=$2 WHERE id=$1")
            .bind(fixture.ctx.user.as_ref().unwrap().id)
            .bind(frozen)
            .execute(&fixture.ctx.pool)
            .await?;
    }
    for path in [
        "/api/v1/workload-hosts/custody/reserve",
        "/api/v1/workload-hosts/custody/prepare",
    ] {
        let body = if path.ends_with("reserve") {
            serde_json::to_value(&prepare.reservation)?
        } else {
            serde_json::to_value(&prepare)?
        };
        fixture
            .ctx
            .post(path, &body, Some(host.session.token.as_str()))
            .await?
            .assert_status(StatusCode::FORBIDDEN);
    }
    let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_delivery_intent")
        .fetch_one(&fixture.ctx.pool)
        .await?;
    assert_eq!(intents, 0);
    // Restored authority can separately reserve a new exact version/custody.
    let old = ArtifactVersionRepository::find_by_id(&fixture.ctx.pool, fixture.binding.version)
        .await?
        .unwrap();
    let artifact = ArtifactRepository::find_by_id(&fixture.ctx.pool, old.artifact)
        .await?
        .unwrap();
    let mut tx = fixture.ctx.pool.begin().await?;
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
    fixture
        .ctx
        .post(
            "/api/v1/workload-hosts/custody/reserve",
            &new,
            Some(host.session.token.as_str()),
        )
        .await?
        .assert_status(StatusCode::OK);
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
async fn legacy_settlement_waits_for_admitted_remote_preparation_commit() -> Result<()> {
    cancellation_waits_for_admitted_preparation(true).await
}

async fn cancellation_waits_for_admitted_preparation(legacy: bool) -> Result<()> {
    use attune_common::log_custody::CustodyAbsence;
    let fixture = LiveCustody::new().await?;
    let host = fixture.enroll_host().await?;
    let prepare = fixture.remote_preparation();
    fixture
        .ctx
        .post(
            "/api/v1/workload-hosts/custody/reserve",
            &prepare.reservation,
            Some(host.session.token.as_str()),
        )
        .await?
        .assert_status(StatusCode::OK);
    let mut barrier = fixture.ctx.pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await?;
    let _: i64 = sqlx::query_scalar("SELECT id FROM artifact_version WHERE id=$1 FOR UPDATE")
        .bind(fixture.binding.version)
        .fetch_one(&mut *barrier)
        .await?;
    let mut barrier = Some(barrier);
    let mut tasks = tokio::task::JoinSet::new();
    let app = fixture.ctx.app.clone();
    let request = Request::post("/api/v1/workload-hosts/custody/prepare")
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("Bearer {}", host.session.token.as_str()),
        )
        .body(Body::from(serde_json::to_vec(&prepare)?))?;
    tasks.spawn(
        async move { Ok::<_, std::convert::Infallible>((false, app.oneshot(request).await?)) },
    );
    let exchange = async {
        // This waiter has already acquired shared graph admission, and is
        // blocked inside source preparation, before the operation/intent effect.
        let preparing = custody_waiter(&fixture.ctx.pool, blocker).await?;
        let app = fixture.ctx.app.clone();
        let (path, credential, body) = if legacy {
            (
                ROUTE,
                fixture.tokens.token()?,
                serde_json::to_vec(&fixture.binding)?,
            )
        } else {
            (
                "/api/v1/workload-hosts/custody/cancel-absent",
                host.session.token.as_str().to_owned(),
                serde_json::to_vec(&prepare.reservation)?,
            )
        };
        let request = Request::post(path)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {credential}"))
            .body(Body::from(body))?;
        tasks.spawn(async move {
            Ok::<_, std::convert::Infallible>((true, app.oneshot(request).await?))
        });
        let _cancelling = custody_waiter(&fixture.ctx.pool, preparing).await?;
        barrier
            .take()
            .ok_or("source barrier lost")?
            .commit()
            .await?;
        let mut prepared = false;
        let mut absence = None;
        while let Some(result) = tasks.join_next().await {
            let (cancel, response) = result??;
            if response.status() != StatusCode::OK {
                return Err("custody race request rejected".into());
            }
            if cancel {
                let body = axum::body::to_bytes(response.into_body(), 4096).await?;
                absence = Some(if legacy {
                    match serde_json::from_slice::<ControlResponse>(&body)? {
                        ControlResponse::Waiting => false,
                        _ => {
                            return Err(
                                "legacy control settled or delegated pending preparation".into()
                            )
                        }
                    }
                } else {
                    serde_json::from_slice::<CustodyAbsence>(&body)?.absent
                });
            } else {
                prepared = true;
            }
        }
        if !prepared || absence != Some(false) {
            return Err("absence ACK did not retain committed custody".into());
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    // Every failure path releases its source barrier, then stops/joins request
    // owners before closing the pool. No detached writer or timer survives.
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
        .bind(fixture.binding.custody).fetch_one(&fixture.ctx.pool).await?;
    assert_eq!(state, "prepared");
    assert_eq!(intents, 1);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn custody_handover_can_cancel_retained_old_reservation_but_never_prepare_it() -> Result<()> {
    use attune_common::{
        log_custody::CustodyAbsence,
        repositories::workload_host::WorkloadHostRepository,
        workload_host::{self, EnrollmentActor, RegisterWorkloadHost},
    };
    let fixture = LiveCustody::new().await?;
    let first = fixture.enroll_host().await?;
    let request = fixture.remote_preparation();
    fixture
        .ctx
        .post(
            "/api/v1/workload-hosts/custody/reserve",
            &request.reservation,
            Some(first.session.token.as_str()),
        )
        .await?
        .assert_status(StatusCode::OK);
    let claims = attune_common::auth::jwt::validate_token(
        first.session.token.as_str(),
        &fixture.ctx.state.jwt_config,
    )?;
    let actor = workload_host::host_actor(&claims)?;
    let second = WorkloadHostRepository::register(
        &fixture.ctx.pool,
        EnrollmentActor {
            identity: actor.identity,
            enrollment: actor.enrollment,
            profile: actor.profile,
            generation: actor.generation,
            expires_at: chrono::Utc::now().timestamp() + 1800,
        },
        &RegisterWorkloadHost {
            instance: Uuid::new_v4(),
            custody_instance: Uuid::new_v4(),
        },
        &fixture.ctx.state.jwt_config,
    )
    .await?;
    // An old registered manager and a replacement with the old entry cannot
    // publish its delayed preparation. The replacement may close retained
    // negative custody using its authenticated CURRENT delivery incarnation.
    for token in [first.session.token.as_str(), second.session.token.as_str()] {
        fixture
            .ctx
            .post(
                "/api/v1/workload-hosts/custody/prepare",
                &request,
                Some(token),
            )
            .await?
            .assert_status(StatusCode::FORBIDDEN);
    }
    let receipt = fixture
        .ctx
        .post(
            "/api/v1/workload-hosts/custody/cancel-absent",
            &request.reservation,
            Some(second.session.token.as_str()),
        )
        .await?
        .assert_status(StatusCode::OK)
        .json::<CustodyAbsence>()
        .await?;
    assert!(receipt.absent);
    let (state,intents):(String,i64)=sqlx::query_as("SELECT state,(SELECT COUNT(*) FROM log_delivery_intent WHERE custody=$1) FROM log_custody_operation WHERE custody=$1")
        .bind(request.reservation.binding.custody).fetch_one(&fixture.ctx.pool).await?;
    assert_eq!(state, "cancelled");
    assert_eq!(intents, 0);
    // A missing operation for another old incarnation does NOT prove absence.
    let mut unreserved = request.reservation.clone();
    unreserved.binding.custody = Uuid::new_v4();
    fixture
        .ctx
        .post(
            "/api/v1/workload-hosts/custody/cancel-absent",
            &unreserved,
            Some(second.session.token.as_str()),
        )
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    fixture.ctx.cleanup().await?;
    Ok(())
}
