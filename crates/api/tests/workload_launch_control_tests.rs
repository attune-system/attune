mod helpers;
use attune_common::{
    auth::jwt::validate_token,
    repositories::{
        delegated_execution::DelegatedExecutionRepository,
        execution::ExecutionRepository,
        workload_launch::{ControllerActor, WorkloadLaunchRepository as Ledger},
        FindById,
    },
    workload_launch::*,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use helpers::{Result, TestContext};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    ctx: TestContext,
    credential: String,
    image: i64,
    spec: LaunchSpec,
}
impl Fixture {
    async fn new() -> Result<Self> {
        attune_common::auth::install_crypto_provider();
        let ctx = TestContext::new().await?.with_admin_auth().await?;
        let owner = ctx.user.as_ref().unwrap().id;
        let source = helpers::delegated_source::token(&ctx, owner, &["standard".into()]).await?;
        let claims = validate_token(&source, &ctx.state.jwt_config)?;
        let execution = claims.metadata.unwrap()["execution_id"].as_i64().unwrap();
        let execution = ExecutionRepository::find_by_id(&ctx.pool, execution)
            .await?
            .unwrap();
        let worker = execution.worker.unwrap();
        let instance = Uuid::new_v4();
        sqlx::query("UPDATE worker SET meta=COALESCE(meta,'{}'::jsonb)||jsonb_build_object('registration_incarnation',$2::text) WHERE id=$1")
            .bind(worker).bind(instance.to_string()).execute(&ctx.pool).await?;
        let mut tx = ctx.pool.begin().await?;
        let attempt = DelegatedExecutionRepository::current_attempt(&mut tx, execution.id).await?;
        tx.commit().await?;
        let controller_access = ctx
            .create_test_user(&format!("controller_{}", Uuid::new_v4().simple()))
            .await?;
        let controller_identity = validate_token(&controller_access, &ctx.state.jwt_config)?
            .sub
            .parse::<i64>()?;
        let engine:i64=sqlx::query_scalar("INSERT INTO workload_engine(pool,controller_url) VALUES($1,'https://owned-controller.invalid') RETURNING id")
            .bind(format!("owned-{}",Uuid::new_v4())).fetch_one(&ctx.pool).await?;
        let actor_incarnation = Uuid::new_v4();
        let controller:i64=sqlx::query_scalar("INSERT INTO workload_controller(engine,identity,incarnation) VALUES($1,$2,$3) RETURNING id")
            .bind(engine).bind(controller_identity).bind(actor_incarnation).fetch_one(&ctx.pool).await?;
        let actor = ControllerActor {
            controller,
            engine,
            identity: controller_identity,
            incarnation: actor_incarnation,
            expires_at: chrono::Utc::now().timestamp() + 300,
        };
        let credential = attune_common::workload_controller::mint(actor, &ctx.state.jwt_config)?;
        let digest = Sha256Digest::of_bytes(b"owned-image-fixture");
        let reference = format!("sha256:{}", digest.as_str());
        let image:i64=sqlx::query_scalar("INSERT INTO workload_execution_image(engine,reference,sha256,platform) VALUES($1,$2,$3,'linux/amd64') RETURNING id")
            .bind(engine).bind(&reference).bind(digest.as_str()).fetch_one(&ctx.pool).await?;
        let spec = LaunchSpec {
            manifest: LaunchManifest {
                launch_id: 1,
                purpose: LaunchPurpose::Action,
                target: LaunchTarget::Execution {
                    execution: execution.id,
                    worker,
                    worker_instance: instance,
                    attempt,
                },
                owner_identity_id: owner,
                engine_id: engine,
                engine_name_nonce: Uuid::new_v4(),
                image: PinnedImage {
                    reference,
                    sha256: digest.clone(),
                },
                context: FrozenContextRef {
                    id: 1,
                    version: 1,
                    ciphertext_sha256: digest,
                },
                inputs: vec![],
                pack_access: PackAccess::ReadOnly,
                stage: None,
                timeout_ms: Some(1000),
                lease_deadline: chrono::Utc::now() + chrono::Duration::minutes(1),
            },
            invocation: LaunchInvocation {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "owned-private-context".into(),
                ],
                env: BTreeMap::from([("PRIVATE_VALUE".into(), "owned-private-context".into())]),
                working_root: LaunchRoot::Scratch,
            },
        };
        Ok(Self {
            ctx,
            credential,
            image,
            spec,
        })
    }
    fn master(&self) -> &str {
        self.ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap()
    }
    async fn reserve(&self) -> Result<ReservedLaunch> {
        Ok(Ledger::freeze(&self.ctx.pool, self.image, self.spec.clone(), self.master()).await?)
    }
    async fn call<T: Serialize>(
        &self,
        id: i64,
        operation: &str,
        body: &T,
        header: &str,
        token: &LaunchToken,
    ) -> Result<(StatusCode, Value)> {
        self.request(
            &format!("/api/v1/workload-launches/{id}/{operation}"),
            body,
            Some(&self.credential),
            Some((header, token.as_str())),
        )
        .await
    }
    async fn request<T: Serialize>(
        &self,
        path: &str,
        body: &T,
        auth: Option<&str>,
        extra: Option<(&str, &str)>,
    ) -> Result<(StatusCode, Value)> {
        let mut builder = Request::post(path).header("content-type", "application/json");
        if let Some(auth) = auth {
            builder = builder.header("authorization", format!("Bearer {auth}"));
        }
        if let Some((name, value)) = extra {
            builder = builder.header(name, value);
        }
        let response = self
            .ctx
            .app
            .clone()
            .oneshot(builder.body(Body::from(serde_json::to_vec(body)?))?)
            .await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        let value = serde_json::from_slice(&bytes)?;
        Ok((status, value))
    }
    async fn ok<T: Serialize, R: DeserializeOwned>(
        &self,
        reserved: &ReservedLaunch,
        operation: &str,
        body: &T,
        header: &str,
        token: &LaunchToken,
    ) -> Result<R> {
        let (status, value) = self
            .call(reserved.request.launch_id, operation, body, header, token)
            .await?;
        assert_eq!(status, StatusCode::OK, "unexpected response: {value}");
        Ok(serde_json::from_value(value)?)
    }
}
#[tokio::test]
async fn admitted_action_one_shot_create_start_exit_removal_and_discovery() -> Result<()> {
    let fixture = Fixture::new().await?;
    let reserved = fixture.reserve().await?;
    let row: (String, String) =
        sqlx::query_as("SELECT manifest::text,encrypted_context FROM workload_launch WHERE id=$1")
            .bind(reserved.request.launch_id)
            .fetch_one(&fixture.ctx.pool)
            .await?;
    assert!(!row.0.contains("owned-private-context"));
    assert!(!row.1.contains("owned-private-context"));
    let (status, _) = fixture
        .request(
            &format!(
                "/api/v1/workload-launches/{}/resolve",
                reserved.request.launch_id
            ),
            &reserved.request,
            None,
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = fixture
        .request(
            &format!(
                "/api/v1/workload-launches/{}/resolve",
                reserved.request.launch_id
            ),
            &reserved.request,
            fixture.ctx.token(),
            Some((LAUNCH_DISPATCH_HEADER, reserved.dispatch_token.as_str())),
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let claimed: ClaimedLaunch = fixture
        .ok(
            &reserved,
            "claim",
            &reserved.request,
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    let (manifest, lease) = match claimed {
        ClaimedLaunch::Active { spec, lease } => {
            assert_eq!(
                spec.invocation.env["PRIVATE_VALUE"],
                "owned-private-context"
            );
            (spec.manifest, lease)
        }
        _ => panic!("expected active claim"),
    };
    let request = LaunchLeaseRequest {
        handle: lease.handle.clone(),
    };
    let intent: LaunchCreateIntent = fixture
        .ok(
            &reserved,
            "create-intent",
            &request,
            LAUNCH_LEASE_HEADER,
            &lease.token,
        )
        .await?;
    assert_eq!(intent.disposition, CreateDisposition::CreateOnce);
    let retry: LaunchCreateIntent = fixture
        .ok(
            &reserved,
            "create-intent",
            &request,
            LAUNCH_LEASE_HEADER,
            &lease.token,
        )
        .await?;
    assert_eq!(retry.disposition, CreateDisposition::ReconcileOnly);
    assert_eq!(intent.create_nonce, retry.create_nonce);
    let resource = EngineResourceEvidence {
        engine_id: manifest.engine_id,
        engine_name_nonce: manifest.engine_name_nonce,
        resource_id: "owned-docker-container".into(),
        admitted_digest: reserved.request.admitted_digest.clone(),
        public_manifest_digest: Sha256Digest::of_bytes(b"owned-original-policy"),
    };
    let observed: LaunchStatus = fixture
        .ok(
            &reserved,
            "observe",
            &ObserveLaunchRequest {
                handle: lease.handle.clone(),
                observation: LaunchObservation::Created {
                    create_nonce: intent.create_nonce,
                    resource: resource.clone(),
                },
            },
            LAUNCH_LEASE_HEADER,
            &lease.token,
        )
        .await?;
    assert_eq!(observed.state, LaunchState::Created);
    let start: LaunchStartIntent = fixture
        .ok(
            &reserved,
            "start",
            &request,
            LAUNCH_LEASE_HEADER,
            &lease.token,
        )
        .await?;
    assert_eq!(start.disposition, StartDisposition::StartOnce);
    let exited: LaunchStatus = fixture
        .ok(
            &reserved,
            "observe",
            &ObserveLaunchRequest {
                handle: lease.handle.clone(),
                observation: LaunchObservation::Exited {
                    start_nonce: start.start_nonce,
                    resource: resource.clone(),
                    exit_code: 0,
                    oom_killed: false,
                },
            },
            LAUNCH_LEASE_HEADER,
            &lease.token,
        )
        .await?;
    assert_eq!(exited.state, LaunchState::CleanupRequired);
    assert_eq!(exited.stop_reason, None);
    assert!(!exited.permits_stage_publication());
    // Revoking the image cannot strand an exited container or change the
    // original engine policy used to prove Wait+Remove.
    sqlx::query("UPDATE workload_execution_image SET revoked_at=clock_timestamp() WHERE id=$1")
        .bind(fixture.image)
        .execute(&fixture.ctx.pool)
        .await?;
    let page = ReconcileLaunchesRequest {
        after: None,
        limit: 10,
    };
    let (status, value) = fixture
        .request(
            "/api/v1/workload-launches/reconcile",
            &page,
            Some(&fixture.credential),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let page: ReconcileLaunchesPage = serde_json::from_value(value)?;
    assert_eq!(page.items.len(), 1);
    assert_eq!(
        page.items[0].dispatch_token.as_str(),
        reserved.dispatch_token.as_str()
    );
    let reclaimed: ClaimedLaunch = fixture
        .ok(
            &reserved,
            "claim",
            &reserved.request,
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    let cleanup = match reclaimed {
        ClaimedLaunch::Cleanup { lease, .. } => lease,
        _ => panic!("expected immediate cleanup lease"),
    };
    assert_eq!(cleanup.purpose, LaunchLeasePurpose::Cleanup);
    assert!(cleanup.handle.lease_generation > lease.handle.lease_generation);
    let (stale, _) = fixture
        .call(
            reserved.request.launch_id,
            "renew",
            &request,
            LAUNCH_LEASE_HEADER,
            &lease.token,
        )
        .await?;
    assert_eq!(stale, StatusCode::FORBIDDEN);
    let mut ack = StopLaunchAcknowledgement {
        handle: cleanup.handle.clone(),
        create: ProviderCreateSettlement::Created {
            create_nonce: intent.create_nonce,
            resource: resource.clone(),
        },
        removed_resource: None,
    };
    let (status, _) = fixture
        .call(
            reserved.request.launch_id,
            "stop-ack",
            &ack,
            LAUNCH_LEASE_HEADER,
            &cleanup.token,
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    ack.removed_resource = Some(resource.clone());
    let stopped: LaunchStatus = fixture
        .ok(
            &reserved,
            "stop-ack",
            &ack,
            LAUNCH_LEASE_HEADER,
            &cleanup.token,
        )
        .await?;
    assert_eq!(stopped.state, LaunchState::Stopped);
    assert_eq!(
        stopped.exit_outcome,
        Some(LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false
        })
    );
    assert_eq!(stopped.resource, Some(resource));
    let (status, _) = fixture
        .call(
            reserved.request.launch_id,
            "start",
            &request,
            LAUNCH_LEASE_HEADER,
            &cleanup.token,
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    fixture.ctx.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn reserve_revoke_restore_claim_never_revives_old_launch_but_new_admission_succeeds(
) -> Result<()> {
    let fixture = Fixture::new().await?;
    let old = fixture.reserve().await?;
    let owner = fixture.spec.manifest.owner_identity_id;
    sqlx::query("UPDATE identity SET frozen=true WHERE id=$1")
        .bind(owner)
        .execute(&fixture.ctx.pool)
        .await?;
    sqlx::query("UPDATE identity SET frozen=false WHERE id=$1")
        .bind(owner)
        .execute(&fixture.ctx.pool)
        .await?;
    let claimed: ClaimedLaunch = fixture
        .ok(
            &old,
            "claim",
            &old.request,
            LAUNCH_DISPATCH_HEADER,
            &old.dispatch_token,
        )
        .await?;
    let lease = match claimed {
        ClaimedLaunch::Cleanup { status, lease, .. } => {
            assert_eq!(status.stop_reason, Some(LaunchStopReason::SourceRevoked));
            lease
        }
        _ => panic!("revoked reservation became active"),
    };
    let create: LaunchCreateIntent = fixture
        .ok(
            &old,
            "create-intent",
            &LaunchLeaseRequest {
                handle: lease.handle.clone(),
            },
            LAUNCH_LEASE_HEADER,
            &lease.token,
        )
        .await?;
    assert_eq!(create.disposition, CreateDisposition::StopOnly);
    assert!(create.create_nonce.is_nil());
    let newer = fixture.reserve().await?;
    let claimed: ClaimedLaunch = fixture
        .ok(
            &newer,
            "claim",
            &newer.request,
            LAUNCH_DISPATCH_HEADER,
            &newer.dispatch_token,
        )
        .await?;
    assert!(matches!(claimed, ClaimedLaunch::Active { .. }));
    // A controller token is never an IAM credential for an unrelated route.
    let (status, _) = fixture
        .request(
            "/api/v1/internal/log-delivery/control",
            &json!({}),
            Some(&fixture.credential),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    fixture.ctx.cleanup().await?;
    Ok(())
}
