mod helpers;
use attune_common::{
    auth::jwt::validate_token,
    repositories::{
        delegated_execution::DelegatedExecutionRepository,
        execution::ExecutionRepository,
        workload_launch::{ControllerActor, WorkloadLaunchRepository as Ledger},
        Create, FindById,
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
    archive: Option<OwnedRelease>,
}
struct OwnedRelease {
    _root: tempfile::TempDir,
    path: std::path::PathBuf,
}
impl Fixture {
    async fn new() -> Result<Self> {
        Self::with_real_archive(false).await
    }
    async fn with_real_archive(real: bool) -> Result<Self> {
        attune_common::auth::install_crypto_provider();
        let ctx = TestContext::new().await?.with_admin_auth().await?;
        let owner = ctx.user.as_ref().unwrap().id;
        let (source, archive) = if real {
            let (source, archive) = Self::create_release_source(&ctx, owner).await?;
            (source, Some(archive))
        } else {
            (
                helpers::delegated_source::token(&ctx, owner, &["standard".into()]).await?,
                None,
            )
        };
        let claims = validate_token(&source, &ctx.state.jwt_config)?;
        let execution = claims.metadata.unwrap()["execution_id"].as_i64().unwrap();
        let execution = ExecutionRepository::find_by_id(&ctx.pool, execution)
            .await?
            .unwrap();
        sqlx::query("UPDATE execution SET timeout_seconds=300 WHERE id=$1")
            .bind(execution.id)
            .execute(&ctx.pool)
            .await?;
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
        let engine:i64=sqlx::query_scalar("INSERT INTO workload_engine(pool,controller_url,daemon_id,platform) VALUES($1,'https://owned-controller.invalid','owned-daemon','linux/amd64') RETURNING id")
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
            archive,
        })
    }
    async fn create_release_source(
        ctx: &TestContext,
        owner: i64,
    ) -> Result<(String, OwnedRelease)> {
        use attune_common::{
            pack_format,
            repositories::{
                component_lifecycle::PackProjectionIds,
                pack_release::{CreatePackReleaseInput, PackReleaseRepository},
            },
        };
        let reference = format!("owned_release_{}", Uuid::new_v4().simple());
        let pack = helpers::create_test_pack(&ctx.pool, &reference).await?;
        let action_ref = format!("{reference}.source");
        let action =
            helpers::create_test_action(&ctx.pool, pack.id, &reference, &action_ref).await?;
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        std::fs::create_dir_all(source.join("actions"))?;
        std::fs::write(
            source.join("pack.yaml"),
            format!("ref: {reference}\nversion: {}\n", pack.version),
        )?;
        std::fs::write(
            source.join("actions/source.yaml"),
            format!("ref: {action_ref}\nrunner_type: python\nentry_point: main.py\n"),
        )?;
        std::fs::write(source.join("actions/main.py"), b"print('owned')\n")?;
        let path = root.path().join("release.tar.gz");
        let release = pack_format::build(
            &pack_format::BuildOptions {
                source: source.clone(),
                artifacts: vec![],
                executables: vec![],
            },
            &path,
            pack_format::Limits::default(),
        )?;
        let mut tx = ctx.pool.begin().await?;
        let row = PackReleaseRepository::create_or_get(
            &mut tx,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: reference,
                version: pack.version,
                digest: release.sha256.clone(),
                object_key: format!("owned-pack/{}", Uuid::new_v4()),
                provider_version: "e:owned-release".into(),
                content_path: source.to_string_lossy().into_owned(),
                archive_size: release.compressed_size.try_into()?,
                manifest: serde_json::to_value(release.manifest())?,
            },
        )
        .await?;
        PackReleaseRepository::activate_projected(
            &mut tx,
            pack.id,
            row.id,
            &PackProjectionIds {
                actions: vec![action.id],
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await?;
        let execution = helpers::delegated_source::create(
            &ctx.pool,
            attune_common::repositories::execution::CreateExecutionInput {
                action: Some(action.id),
                action_ref,
                executor: Some(owner),
                permission_set_refs: vec!["standard".into()],
                ..Default::default()
            },
        )
        .await?;
        let token = helpers::delegated_source::execution_token(ctx, execution.id).await?;
        Ok((token, OwnedRelease { _root: root, path }))
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
    async fn running(&self) -> Result<(ReservedLaunch, LaunchLease)> {
        let reserved = self.reserve().await?;
        let claim: ClaimedLaunch = self
            .ok(
                &reserved,
                "claim",
                &reserved.request,
                LAUNCH_DISPATCH_HEADER,
                &reserved.dispatch_token,
            )
            .await?;
        let ClaimedLaunch::Active { spec, lease } = claim else {
            return Err("active claim required".into());
        };
        let request = LaunchLeaseRequest {
            handle: lease.handle.clone(),
        };
        let intent: LaunchCreateIntent = self
            .ok(
                &reserved,
                "create-intent",
                &request,
                LAUNCH_LEASE_HEADER,
                &lease.token,
            )
            .await?;
        let resource = EngineResourceEvidence {
            engine_id: spec.manifest.engine_id,
            engine_name_nonce: spec.manifest.engine_name_nonce,
            resource_id: "owned-artifact-fixture".into(),
            admitted_digest: reserved.request.admitted_digest.clone(),
            public_manifest_digest: Sha256Digest::of_bytes(b"owned-artifact-policy"),
        };
        let _: LaunchStatus = self
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
        let start: LaunchStartIntent = self
            .ok(
                &reserved,
                "start",
                &request,
                LAUNCH_LEASE_HEADER,
                &lease.token,
            )
            .await?;
        let status: LaunchStatus = self
            .ok(
                &reserved,
                "observe",
                &ObserveLaunchRequest {
                    handle: lease.handle.clone(),
                    observation: LaunchObservation::Started {
                        start_nonce: start.start_nonce,
                        resource,
                    },
                },
                LAUNCH_LEASE_HEADER,
                &lease.token,
            )
            .await?;
        assert_eq!(status.runtime_state, LaunchRuntimeState::Running);
        Ok((reserved, lease))
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
    assert!(
        fixture.reserve().await.is_err(),
        "restoration cannot allocate another launch for the old attempt"
    );
    let mut fresh = fixture.spec.clone();
    fresh.manifest.purpose = LaunchPurpose::RuntimeProbe;
    fresh.manifest.target = Ledger::prerequisite(
        &fixture.ctx.pool,
        &fixture.spec.manifest.target,
        fixture.spec.manifest.owner_identity_id,
        fixture.image,
        LaunchPurpose::RuntimeProbe,
        None,
        chrono::Utc::now() + chrono::Duration::minutes(5),
    )
    .await?;
    let newer = Ledger::freeze(&fixture.ctx.pool, fixture.image, fresh, fixture.master()).await?;
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

#[tokio::test]
async fn concurrent_reservation_replay_keeps_one_identity_and_rejects_changed_plan() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (first, replay) = tokio::join!(fixture.reserve(), fixture.reserve());
    let first = first?;
    let replay = replay?;
    assert_eq!(first.request, replay.request);
    assert_eq!(
        first.dispatch_token.as_str(),
        replay.dispatch_token.as_str()
    );
    let mut changed = fixture.spec.clone();
    changed.invocation.argv.push("changed-invocation".into());
    assert!(
        Ledger::freeze(&fixture.ctx.pool, fixture.image, changed, fixture.master())
            .await
            .is_err()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workload_launch WHERE manifest->'target'=$1")
            .bind(sqlx::types::Json(&fixture.spec.manifest.target))
            .fetch_one(&fixture.ctx.pool)
            .await?;
    assert_eq!(count, 1);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn engine_identity_is_immutable_and_disable_restore_permanently_closes_old_launch(
) -> Result<()> {
    let fixture = Fixture::new().await?;
    let reserved = fixture.reserve().await?;
    for (credential, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (fixture.ctx.token(), StatusCode::FORBIDDEN),
        (Some(fixture.credential.as_str()), StatusCode::OK),
    ] {
        let mut request = Request::get("/api/v1/workload-launches/engine");
        if let Some(credential) = credential {
            request = request.header("authorization", format!("Bearer {credential}"));
        }
        let response = fixture
            .ctx
            .app
            .clone()
            .oneshot(request.body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), expected);
        if expected == StatusCode::OK {
            let body = to_bytes(response.into_body(), 4096).await?;
            let binding: LaunchEngineBinding = serde_json::from_slice(&body)?;
            assert_eq!(binding.engine_id, fixture.actor().engine);
            assert_eq!(binding.daemon_id, "owned-daemon");
            assert_eq!(binding.platform, LaunchPlatform::LinuxAmd64);
        }
    }
    for sql in [
        "UPDATE workload_engine SET daemon_id='replacement-daemon' WHERE id=$1",
        "UPDATE workload_engine SET platform='linux/arm64' WHERE id=$1",
        "DELETE FROM workload_engine WHERE id=$1",
    ] {
        assert!(sqlx::query(sql)
            .bind(fixture.actor().engine)
            .execute(&fixture.ctx.pool)
            .await
            .is_err());
    }
    for disabled in [true, false] {
        sqlx::query("UPDATE workload_engine SET disabled=$2 WHERE id=$1")
            .bind(fixture.actor().engine)
            .bind(disabled)
            .execute(&fixture.ctx.pool)
            .await?;
    }
    let claimed: ClaimedLaunch = fixture
        .ok(
            &reserved,
            "claim",
            &reserved.request,
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    assert!(matches!(
        claimed,
        ClaimedLaunch::Cleanup {
            status: LaunchStatus {
                stop_reason: Some(LaunchStopReason::SourceRevoked),
                ..
            },
            ..
        }
    ));
    // Engine admission revocation does not erase the cleanup identity.
    let binding = Ledger::engine_binding(&fixture.ctx.pool, fixture.actor()).await?;
    assert_eq!(binding.daemon_id, "owned-daemon");
    fixture.ctx.cleanup().await?;
    Ok(())
}

fn archive(name: &str, contents: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(
        name,
        zip::write::SimpleFileOptions::default().unix_permissions(0o755),
    )
    .unwrap();
    zip.write_all(contents).unwrap();
    zip.finish().unwrap().into_inner()
}
impl Fixture {
    fn actor(&self) -> ControllerActor {
        attune_common::workload_controller::actor(
            &validate_token(&self.credential, &self.ctx.state.jwt_config).unwrap(),
        )
        .unwrap()
    }
    async fn setup(
        &mut self,
    ) -> Result<(
        LaunchSpec,
        attune_common::repositories::workload_launch::RuntimeStageBinding,
    )> {
        use attune_common::{
            blob_store::{body_from_bytes, sha256, ObjectKey},
            repositories::workload_launch::RuntimeStageBinding,
        };
        let mut action = self.spec.clone();
        let pack = archive("pack.yaml", b"ref: owned-fixture\n");
        let key = ObjectKey::new(format!("workload-input/{}", Uuid::new_v4().simple()))?;
        let stored = self
            .ctx
            .state
            .blob_store
            .put_file_atomic(
                &key,
                body_from_bytes(pack.clone().into()),
                pack.len() as u64,
                sha256(&pack),
            )
            .await?;
        let digest = Sha256Digest::of_bytes(&pack);
        let object:i64=sqlx::query_scalar("INSERT INTO workload_launch_object(source_target,kind,object_key,provider_version,size_bytes,sha256,max_expanded_bytes,max_files) VALUES($1,'pack',$2,$3,$4,$5,1048576,32) RETURNING id")
            .bind(sqlx::types::Json(&action.manifest.target)).bind(key.as_str()).bind(stored.provider_version.as_stored()).bind(pack.len() as i64).bind(digest.as_str()).fetch_one(&self.ctx.pool).await?;
        let pin = PinnedLaunchInput {
            pin_id: object,
            object_id: object,
            kind: LaunchInputKind::Pack,
            object_version: stored.provider_version.as_stored().into(),
            size_bytes: pack.len() as u64,
            sha256: digest.clone(),
            max_expanded_bytes: 1048576,
            max_files: 32,
        };
        action.manifest.inputs = vec![pin.clone()];
        let binding = RuntimeStageBinding {
            engine_id: action.manifest.engine_id,
            platform: LaunchPlatform::LinuxAmd64,
            image_sha256: action.manifest.image.sha256.clone(),
            pack_sha256: digest,
            configuration_ciphertext_sha256: Sha256Digest::of_bytes(
                &attune_common::crypto::random_bytes::<32>(),
            ),
            runtime_definition_sha256: Sha256Digest::of_bytes(b"owned-public-runtime-definition"),
        };
        self.spec.manifest.purpose = LaunchPurpose::Setup;
        self.spec.manifest.target = Ledger::prerequisite(
            &self.ctx.pool,
            &action.manifest.target,
            action.manifest.owner_identity_id,
            self.image,
            LaunchPurpose::Setup,
            Some(binding.clone()),
            chrono::Utc::now() + chrono::Duration::minutes(5),
        )
        .await?;
        self.spec.manifest.inputs = vec![pin];
        self.spec.manifest.stage = Some(PrivateStage {
            cache_key: "caller-placeholder-must-be-replaced".into(),
            quota_bytes: 1048576,
            quota_files: 32,
        });
        self.spec.invocation.working_root = LaunchRoot::Stage;
        Ok((action, binding))
    }
    async fn removed_setup(&self, reserved: &ReservedLaunch) -> Result<LaunchLease> {
        let active = Ledger::claim(
            &self.ctx.pool,
            self.actor(),
            &reserved.request,
            &reserved.dispatch_token,
            self.master(),
        )
        .await?;
        let (manifest, lease) = match active {
            ClaimedLaunch::Active { spec, lease } => (spec.manifest, lease),
            _ => panic!("expected active setup"),
        };
        assert_ne!(
            manifest.stage.as_ref().unwrap().cache_key,
            "caller-placeholder-must-be-replaced"
        );
        let create = Ledger::create_intent(
            &self.ctx.pool,
            self.actor(),
            &LaunchLeaseRequest {
                handle: lease.handle.clone(),
            },
            &lease.token,
        )
        .await?;
        let resource = EngineResourceEvidence {
            engine_id: manifest.engine_id,
            engine_name_nonce: manifest.engine_name_nonce,
            resource_id: "owned-setup-container".into(),
            admitted_digest: reserved.request.admitted_digest.clone(),
            public_manifest_digest: Sha256Digest::of_bytes(b"owned-fixed-engine-policy"),
        };
        Ledger::observe(
            &self.ctx.pool,
            self.actor(),
            &ObserveLaunchRequest {
                handle: lease.handle.clone(),
                observation: LaunchObservation::Created {
                    create_nonce: create.create_nonce,
                    resource: resource.clone(),
                },
            },
            &lease.token,
        )
        .await?;
        let start = Ledger::start_intent(
            &self.ctx.pool,
            self.actor(),
            &LaunchLeaseRequest {
                handle: lease.handle.clone(),
            },
            &lease.token,
        )
        .await?;
        Ledger::observe(
            &self.ctx.pool,
            self.actor(),
            &ObserveLaunchRequest {
                handle: lease.handle.clone(),
                observation: LaunchObservation::Exited {
                    start_nonce: start.start_nonce,
                    resource: resource.clone(),
                    exit_code: 0,
                    oom_killed: false,
                },
            },
            &lease.token,
        )
        .await?;
        let cleanup = Ledger::claim(
            &self.ctx.pool,
            self.actor(),
            &reserved.request,
            &reserved.dispatch_token,
            self.master(),
        )
        .await?;
        let lease = match cleanup {
            ClaimedLaunch::Cleanup { lease, .. } => lease,
            _ => panic!("expected immediate cleanup"),
        };
        Ledger::stop_ack(
            &self.ctx.pool,
            self.actor(),
            &StopLaunchAcknowledgement {
                handle: lease.handle.clone(),
                create: ProviderCreateSettlement::Created {
                    create_nonce: create.create_nonce,
                    resource: resource.clone(),
                },
                removed_resource: Some(resource),
            },
            &lease.token,
        )
        .await?;
        let publication = Ledger::claim(
            &self.ctx.pool,
            self.actor(),
            &reserved.request,
            &reserved.dispatch_token,
            self.master(),
        )
        .await?;
        Ok(match publication {
            ClaimedLaunch::Publication { lease, .. } => lease,
            _ => panic!("expected publication-only recovery"),
        })
    }
    async fn upload_archive(
        &self,
        reserved: &ReservedLaunch,
        lease: &LaunchLease,
        grant: &LaunchOutputGrant,
        bytes: Vec<u8>,
    ) -> Result<(StatusCode, Value)> {
        let path = format!(
            "/api/v1/workload-launches/{}/outputs/{}/body?admitted_digest={}",
            reserved.request.launch_id,
            grant.output_id,
            reserved.request.admitted_digest.as_str()
        );
        let request = Request::post(path)
            .header("authorization", format!("Bearer {}", self.credential))
            .header("content-type", LAUNCH_ARCHIVE_CONTENT_TYPE)
            .header("content-length", bytes.len().to_string())
            .header(LAUNCH_LEASE_HEADER, lease.token.as_str())
            .header(LAUNCH_OUTPUT_HEADER, grant.token.as_str())
            .body(Body::from(bytes))?;
        let response = self.ctx.app.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }
}
#[tokio::test]
async fn removed_setup_publication_replays_exact_receipt_and_requires_real_build_image_binding(
) -> Result<()> {
    let mut fixture = Fixture::new().await?;
    let (mut action, binding) = fixture.setup().await?;
    let reserved = fixture.reserve().await?;
    let publication = fixture.removed_setup(&reserved).await?;
    let bytes = archive("bin/helper", b"#!/bin/sh\nprintf owned\n");
    let request = ReserveLaunchOutput {
        handle: publication.handle.clone(),
        kind: LaunchOutputKind::StageArchive,
        size_bytes: bytes.len() as u64,
        sha256: Sha256Digest::of_bytes(&bytes),
    };
    let grant = Ledger::reserve_output(
        &fixture.ctx.pool,
        fixture.actor(),
        &request,
        &publication.token,
        fixture.master(),
    )
    .await?;
    let (status, result) = fixture
        .upload_archive(&reserved, &publication, &grant, bytes.clone())
        .await?;
    assert_eq!(status, StatusCode::OK, "{result}");
    let result: LaunchStageResult = serde_json::from_value(result)?;
    let replay: LaunchStageResult = fixture
        .ok(
            &reserved,
            "stage-result",
            &reserved.request,
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    assert_eq!(result.output_id, replay.output_id);
    assert_eq!(result.validation_receipt, replay.validation_receipt);
    assert_eq!(result.object_version, replay.object_version);
    let (status, again) = fixture
        .upload_archive(&reserved, &publication, &grant, bytes)
        .await?;
    assert_eq!(status, StatusCode::OK, "{again}");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workload_launch_object WHERE build_launch=$1")
            .bind(reserved.request.launch_id)
            .fetch_one(&fixture.ctx.pool)
            .await?;
    assert_eq!(count, 1);
    action.manifest.inputs.push(PinnedLaunchInput {
        pin_id: result.object_id,
        object_id: result.object_id,
        kind: LaunchInputKind::Runtime,
        object_version: result.object_version.clone(),
        size_bytes: result.size_bytes,
        sha256: result.sha256.clone(),
        max_expanded_bytes: 1048576,
        max_files: 32,
    });
    assert!(
        Ledger::freeze(
            &fixture.ctx.pool,
            fixture.image,
            action.clone(),
            fixture.master()
        )
        .await
        .is_err(),
        "manifest co-admission is not build proof"
    );
    Ledger::freeze_with_runtime_binding(
        &fixture.ctx.pool,
        fixture.image,
        action.clone(),
        fixture.master(),
        Some(&binding),
    )
    .await?;
    for (engine_id, platform) in [
        (binding.engine_id + 1, binding.platform),
        (binding.engine_id, LaunchPlatform::LinuxArm64),
    ] {
        let mut wrong = binding.clone();
        wrong.engine_id = engine_id;
        wrong.platform = platform;
        assert!(
            Ledger::freeze_with_runtime_binding(
                &fixture.ctx.pool,
                fixture.image,
                action.clone(),
                fixture.master(),
                Some(&wrong)
            )
            .await
            .is_err(),
            "an index digest alone is not engine/platform build proof"
        );
    }
    let mut wrong = binding.clone();
    wrong.runtime_definition_sha256 = Sha256Digest::of_bytes(b"different-runtime-definition");
    assert!(Ledger::freeze_with_runtime_binding(
        &fixture.ctx.pool,
        fixture.image,
        action.clone(),
        fixture.master(),
        Some(&wrong)
    )
    .await
    .is_err());
    let other = Sha256Digest::of_bytes(b"different-owned-image");
    let reference = format!("sha256:{}", other.as_str());
    let image:i64=sqlx::query_scalar("INSERT INTO workload_execution_image(engine,reference,sha256,platform) VALUES($1,$2,$3,'linux/amd64') RETURNING id").bind(fixture.actor().engine).bind(&reference).bind(other.as_str()).fetch_one(&fixture.ctx.pool).await?;
    action.manifest.image = PinnedImage {
        reference,
        sha256: other.clone(),
    };
    wrong = binding.clone();
    wrong.image_sha256 = other;
    assert!(
        Ledger::freeze_with_runtime_binding(
            &fixture.ctx.pool,
            image,
            action,
            fixture.master(),
            Some(&wrong)
        )
        .await
        .is_err(),
        "runtime built for another image must be denied"
    );
    let owner = fixture.spec.manifest.owner_identity_id;
    sqlx::query("UPDATE identity SET frozen=true WHERE id=$1")
        .bind(owner)
        .execute(&fixture.ctx.pool)
        .await?;
    sqlx::query("UPDATE identity SET frozen=false WHERE id=$1")
        .bind(owner)
        .execute(&fixture.ctx.pool)
        .await?;
    assert!(
        fixture.reserve().await.is_err(),
        "old setup ticket cannot be reused after revoke/restore"
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn controller_session_renewal_retains_exact_target_and_rejects_withdrawal() -> Result<()> {
    use attune_common::workload_session::WorkloadSession;
    let fixture = Fixture::new().await?;
    let (status, value) = fixture
        .request(
            "/api/v1/workload-launches/session",
            &json!({}),
            Some(&fixture.credential),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{value}");
    let renewed: WorkloadSession = serde_json::from_value(value)?;
    let claims = validate_token(renewed.token.as_str(), &fixture.ctx.state.jwt_config)?;
    let actor = attune_common::workload_controller::actor(&claims)?;
    let original = fixture.actor();
    assert_eq!(
        (
            actor.controller,
            actor.identity,
            actor.engine,
            actor.incarnation
        ),
        (
            original.controller,
            original.identity,
            original.engine,
            original.incarnation
        )
    );
    assert_eq!(actor.expires_at, renewed.expires_at.timestamp());
    assert!(actor.expires_at <= chrono::Utc::now().timestamp() + 300);
    sqlx::query("UPDATE workload_controller SET revoked_at=clock_timestamp() WHERE id=$1")
        .bind(original.controller)
        .execute(&fixture.ctx.pool)
        .await?;
    let (status, _) = fixture
        .request(
            "/api/v1/workload-launches/session",
            &json!({}),
            Some(renewed.token.as_str()),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn private_stdin_is_bound_to_frozen_context_and_never_returned_to_controller() -> Result<()> {
    let fixture = Fixture::new().await?;
    let input = String::from("{\"message\":\"owned-private-stdin-value\"}");
    let reserved = Ledger::freeze_with_input_binding(
        &fixture.ctx.pool,
        fixture.image,
        fixture.spec.clone(),
        fixture.master(),
        None,
        Some(input.clone()),
    )
    .await?;
    let (manifest, encrypted): (String, String) =
        sqlx::query_as("SELECT manifest::text,encrypted_context FROM workload_launch WHERE id=$1")
            .bind(reserved.request.launch_id)
            .fetch_one(&fixture.ctx.pool)
            .await?;
    assert!(!manifest.contains("owned-private-stdin-value"));
    assert!(!encrypted.contains("owned-private-stdin-value"));
    assert!(
        !manifest.contains(Sha256Digest::of_bytes(input.as_bytes()).as_str()),
        "no public plaintext-input digest"
    );
    let replay = Ledger::freeze_with_input_binding(
        &fixture.ctx.pool,
        fixture.image,
        fixture.spec.clone(),
        fixture.master(),
        None,
        Some("{ \"message\" : \"owned-private-stdin-value\" }".into()),
    )
    .await?;
    assert_eq!(reserved.request, replay.request);
    assert!(Ledger::freeze_with_input_binding(
        &fixture.ctx.pool,
        fixture.image,
        fixture.spec.clone(),
        fixture.master(),
        None,
        Some("{\"message\":\"changed\"}".into())
    )
    .await
    .is_err());
    let (status, body) = fixture
        .call(
            reserved.request.launch_id,
            "claim",
            &reserved.request,
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains("owned-private-stdin-value"));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn action_preparation_reservation_cannot_revive_after_owner_restoration() -> Result<()> {
    let fixture = Fixture::new().await?;
    let owner = fixture.spec.manifest.owner_identity_id;
    let admission = Ledger::reserve_action_admission(
        &fixture.ctx.pool,
        &fixture.spec.manifest.target,
        owner,
        fixture.image,
    )
    .await?;
    // Preparation has not begun. A complete intervening freeze/restoration
    // must remain visible to this reservation, even though current IAM allows it.
    for frozen in [true, false] {
        sqlx::query("UPDATE identity SET frozen=$2 WHERE id=$1")
            .bind(owner)
            .bind(frozen)
            .execute(&fixture.ctx.pool)
            .await?;
    }
    assert!(Ledger::freeze_admitted_input(
        &fixture.ctx.pool,
        admission,
        fixture.image,
        fixture.spec.clone(),
        fixture.master(),
        None,
        Some("{}".into()),
    )
    .await
    .is_err());
    assert!(Ledger::reserve_action_admission(
        &fixture.ctx.pool,
        &fixture.spec.manifest.target,
        owner,
        fixture.image,
    )
    .await
    .is_err());
    let launches: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workload_launch")
        .fetch_one(&fixture.ctx.pool)
        .await?;
    assert_eq!(launches, 0, "old reservation cannot create a launch");
    let deletion = sqlx::query("DELETE FROM workload_launch_prerequisite WHERE id=$1")
        .bind(admission)
        .execute(&fixture.ctx.pool)
        .await;
    assert!(
        deletion.is_err(),
        "retention cannot erase reservation evidence"
    );

    // A different newly admitted execution can use the restored owner. It does
    // not reuse or mutate the old attempt/idempotency key.
    let source =
        helpers::delegated_source::token(&fixture.ctx, owner, &["standard".into()]).await?;
    let claims = validate_token(&source, &fixture.ctx.state.jwt_config)?;
    let execution = claims.metadata.unwrap()["execution_id"].as_i64().unwrap();
    let row = ExecutionRepository::find_by_id(&fixture.ctx.pool, execution)
        .await?
        .unwrap();
    let worker = row.worker.unwrap();
    let instance = Uuid::new_v4();
    sqlx::query("UPDATE worker SET meta=COALESCE(meta,'{}'::jsonb)||jsonb_build_object('registration_incarnation',$2::text) WHERE id=$1")
        .bind(worker).bind(instance.to_string()).execute(&fixture.ctx.pool).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let attempt = DelegatedExecutionRepository::current_attempt(&mut tx, execution).await?;
    tx.commit().await?;
    let target = LaunchTarget::Execution {
        execution,
        worker,
        worker_instance: instance,
        attempt,
    };
    let fresh =
        Ledger::reserve_action_admission(&fixture.ctx.pool, &target, owner, fixture.image).await?;
    assert_ne!(admission, fresh);
    let mut spec = fixture.spec.clone();
    spec.manifest.target = target;
    Ledger::freeze_admitted_input(
        &fixture.ctx.pool,
        fresh,
        fixture.image,
        spec,
        fixture.master(),
        None,
        Some("{}".into()),
    )
    .await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn run_close_waits_for_file_allocation_and_seals_exact_inventory() -> Result<()> {
    use attune_common::{
        models::{
            ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType,
            RetentionPolicyType,
        },
        repositories::artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
        },
    };
    let fixture = Fixture::new().await?;
    let (reserved, _) = fixture.running().await?;
    let LaunchTarget::Execution { execution, .. } = fixture.spec.manifest.target else {
        unreachable!()
    };
    let artifact = ArtifactRepository::create(
        &fixture.ctx.pool,
        CreateArtifactInput {
            r#ref: format!("owned_{}.file", Uuid::new_v4().simple()),
            scope: OwnerType::Action,
            owner: "owned.action".into(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Days,
            retention_limit: 1,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await?;
    let mut allocation_tx = fixture.ctx.pool.begin().await?;
    let mut proof = Ledger::authorize_artifact_allocation(
        &mut allocation_tx,
        &fixture.spec.manifest.target,
        fixture.spec.manifest.owner_identity_id,
    )
    .await?;
    let allocator_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(proof.connection())
        .await?;
    let version = ArtifactVersionRepository::create_file_backed_with_conn(
        proof.connection(),
        artifact.id,
        &artifact.r#ref,
        "text/plain".into(),
        Some(execution),
        None,
        None,
    )
    .await?;
    let app = fixture.ctx.app.clone();
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/workload-launches/{}/stop",
            reserved.request.launch_id
        ))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", fixture.credential))
        .header(LAUNCH_DISPATCH_HEADER, reserved.dispatch_token.as_str())
        .body(Body::from(serde_json::to_vec(&StopLaunchRequest {
            request: reserved.request.clone(),
            reason: LaunchStopReason::Cancelled,
        })?))?;
    let mut stop = tokio::spawn(async move { app.oneshot(request).await });
    // Observe the actual lock wait, rather than estimating it with a sleep.
    let waiting =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
            ).bind(allocator_pid).fetch_one(&fixture.ctx.pool).await?;
                if blocked {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
    if !matches!(waiting, Ok(Ok(()))) {
        stop.abort();
        let _ = (&mut stop).await;
        allocation_tx.rollback().await?;
        fixture.ctx.cleanup().await?;
        return Err("Run close did not reach the observed allocation fence".into());
    }
    Ledger::bind_artifact_file(&mut proof, version.id, 4096).await?;
    allocation_tx.commit().await?;
    let response = (&mut stop).await??;
    assert_eq!(response.status(), StatusCode::OK);
    let (state, count): (String, i64) = sqlx::query_as(
        "SELECT p.state,(SELECT COUNT(*) FROM workload_artifact_file f WHERE f.publication=p.id) \
         FROM workload_artifact_publication p WHERE p.launch=$1",
    )
    .bind(reserved.request.launch_id)
    .fetch_one(&fixture.ctx.pool)
    .await?;
    assert_eq!((state.as_str(), count), ("pending", 1));
    let mut late = fixture.ctx.pool.begin().await?;
    assert!(Ledger::authorize_artifact_allocation(
        &mut late,
        &fixture.spec.manifest.target,
        fixture.spec.manifest.owner_identity_id
    )
    .await
    .is_err());
    late.rollback().await?;
    let status: String = sqlx::query_scalar("SELECT status::text FROM execution WHERE id=$1")
        .bind(execution)
        .fetch_one(&fixture.ctx.pool)
        .await?;
    assert_eq!(
        status, "running",
        "Run closure independently fences new allocation"
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn public_input_reservation_retains_revocation_and_settles_a_late_put() -> Result<()> {
    use attune_common::{
        blob_store::{settle_atomic_file_put, BlobStore, FilesystemBlobStore},
        pack_format,
        workload_archive::ArchivePolicy,
        workload_pack_input::{release_zip, ReleaseInputPin},
    };
    use futures::StreamExt;
    let fixture = Fixture::with_real_archive(true).await?;
    let archive = &fixture.archive.as_ref().unwrap().path;
    let verified = pack_format::verify(archive, pack_format::Limits::default())?;
    let pin = ReleaseInputPin {
        pack_ref: verified.manifest().pack.r#ref.clone(),
        size_bytes: verified.compressed_size,
        sha256: Sha256Digest::parse(verified.sha256.clone())?,
    };
    let body = release_zip(
        std::fs::File::open(archive)?,
        &pin,
        pack_format::Limits::default(),
        &ArchivePolicy {
            root: LaunchInputKind::Pack,
            max_compressed_bytes: 1024 * 1024,
            max_expanded_bytes: 1024 * 1024,
            max_files: 100,
        },
    )?;
    let root = Ledger::reserve_action_admission(
        &fixture.ctx.pool,
        &fixture.spec.manifest.target,
        fixture.spec.manifest.owner_identity_id,
        fixture.image,
    )
    .await?;
    let permit = Ledger::reserve_pack_input(&fixture.ctx.pool, root, &body).await?;
    let key = permit.key()?;
    let digest: [u8; 32] = hex::decode(permit.sha256().as_str())?.try_into().unwrap();
    let size = permit.size_bytes();
    let bytes = std::fs::read(body.into_file().path())?;
    // The owned request is delayed after its durable reservation. Both source
    // changes commit before the native PUT is released; no sleeps or timeouts.
    let owner = fixture.spec.manifest.owner_identity_id;
    for frozen in [true, false] {
        sqlx::query("UPDATE identity SET frozen=$2 WHERE id=$1")
            .bind(owner)
            .bind(frozen)
            .execute(&fixture.ctx.pool)
            .await?;
    }
    let storage_root = tempfile::tempdir()?;
    let store = FilesystemBlobStore::new(storage_root.path())?;
    let body = futures::stream::once({
        let bytes = bytes.clone();
        async move { Ok(axum::body::Bytes::from(bytes)) }
    })
    .boxed();
    let stored = store.put_file_atomic(&key, body, size, digest).await?;
    assert!(
        Ledger::publish_pack_input(&fixture.ctx.pool, &permit, &stored)
            .await
            .is_err()
    );
    Ledger::reconcile_inputs(&fixture.ctx.pool, 64).await?;
    let plans = Ledger::input_cleanup_candidates(&fixture.ctx.pool, 64).await?;
    let plan = plans
        .iter()
        .find(|plan| plan.key().is_ok_and(|candidate| candidate == key))
        .ok_or("owned cleanup missing")?;
    let settlement = settle_atomic_file_put(&store, &key, plan.token()).await?;
    Ledger::acknowledge_input_cleanup(&fixture.ctx.pool, plan, &settlement).await?;
    // Cleanup acknowledges native settlement, never mere network cancellation.
    // A released duplicate body cannot overwrite the permanent create-only fence.
    let late = futures::stream::once(async move { Ok(axum::body::Bytes::from(bytes)) }).boxed();
    assert!(store
        .put_file_atomic(&key, late, size, digest)
        .await
        .is_err());
    assert!(
        Ledger::publish_pack_input(&fixture.ctx.pool, &permit, &stored)
            .await
            .is_err()
    );
    let (state, settled): (String, bool) = sqlx::query_as(
        "SELECT state,cleanup_settled FROM workload_input_materialization WHERE object_key=$1",
    )
    .bind(key.as_str())
    .fetch_one(&fixture.ctx.pool)
    .await?;
    assert_eq!((state.as_str(), settled), ("cancelled", true));
    let objects: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workload_launch_object WHERE object_key=$1")
            .bind(key.as_str())
            .fetch_one(&fixture.ctx.pool)
            .await?;
    assert_eq!(
        objects, 0,
        "late preparation creates no admitted input object"
    );
    let erased = sqlx::query("DELETE FROM workload_input_materialization WHERE object_key=$1")
        .bind(key.as_str())
        .execute(&fixture.ctx.pool)
        .await;
    assert!(
        erased.is_err(),
        "cleanup cannot erase the old idempotency key"
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn count_only_stdin_authorization_is_denied_even_with_a_live_bound_grant() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (reserved, lease) = fixture.running().await?;
    let grant: LaunchIoGrant = fixture
        .ok(
            &reserved,
            "io-grants",
            &LaunchIoGrantRequest {
                request: reserved.request.clone(),
                channel: LaunchIoChannel::Stdin,
            },
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    for offset in [0, 1, 4] {
        let request = AuthorizeLaunchIoRequest {
            request: reserved.request.clone(),
            channel: LaunchIoChannel::Stdin,
            offset,
            bytes: 4,
        };
        let response = fixture
            .ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/workload-launches/{}/authorize-io",
                        reserved.request.launch_id
                    ))
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", fixture.credential))
                    .header(LAUNCH_LEASE_HEADER, lease.token.as_str())
                    .header(LAUNCH_IO_HEADER, grant.token.as_str())
                    .body(Body::from(serde_json::to_vec(&request)?))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let consumed: bool = sqlx::query_scalar(
        "SELECT (operation->>'stdin_authorized')::boolean FROM workload_launch WHERE id=$1",
    )
    .bind(reserved.request.launch_id)
    .fetch_one(&fixture.ctx.pool)
    .await?;
    assert!(
        !consumed,
        "rejected count/offset requests cannot consume exact stdin authority"
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn artifact_discovery_survives_stopped_complete_capture_without_setup_publication(
) -> Result<()> {
    let fixture = Fixture::new().await?;
    let reserved = fixture.reserve().await?;
    let _: LaunchStatus = fixture
        .ok(
            &reserved,
            "stop",
            &StopLaunchRequest {
                request: reserved.request.clone(),
                reason: LaunchStopReason::Cancelled,
            },
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    let cleanup: ClaimedLaunch = fixture
        .ok(
            &reserved,
            "claim",
            &reserved.request,
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    let ClaimedLaunch::Cleanup { lease, .. } = cleanup else {
        return Err("cleanup claim required".into());
    };
    let stopped: LaunchStatus = fixture
        .ok(
            &reserved,
            "stop-ack",
            &StopLaunchAcknowledgement {
                handle: lease.handle,
                create: ProviderCreateSettlement::NeverSubmitted,
                removed_resource: None,
            },
            LAUNCH_LEASE_HEADER,
            &lease.token,
        )
        .await?;
    assert_eq!(stopped.state, LaunchState::Stopped);
    assert_eq!(
        stopped.capture,
        LaunchCaptureState::Complete,
        "a never-submitted container has no raw output to recover"
    );
    assert_eq!(stopped.publication, LaunchPublicationState::NotRequired);
    let (status, value) = fixture
        .request(
            "/api/v1/workload-launches/reconcile",
            &ReconcileLaunchesRequest {
                after: None,
                limit: 100,
            },
            Some(&fixture.credential),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let page: ReconcileLaunchesPage = serde_json::from_value(value)?;
    assert!(
        page.items
            .iter()
            .any(|item| item.request == reserved.request),
        "independent artifact obligation survives stopped/capture-complete"
    );
    let claimed: ClaimedLaunch = fixture
        .ok(
            &reserved,
            "claim",
            &reserved.request,
            LAUNCH_DISPATCH_HEADER,
            &reserved.dispatch_token,
        )
        .await?;
    assert!(
        matches!(claimed, ClaimedLaunch::Stopped { .. }),
        "artifact discovery must not reopen a Run/Capture lease or private invocation"
    );
    // Only seed the orthogonal terminal artifact receipt. No engine create,
    // file allocation or native upload existed in this zero-resource fixture;
    // upload/custody settlement is not claimed as covered by this test.
    sqlx::query("UPDATE workload_artifact_publication SET state='incomplete',cleanup_receipt=id WHERE launch=$1")
        .bind(reserved.request.launch_id).execute(&fixture.ctx.pool).await?;
    let (status, value) = fixture
        .request(
            "/api/v1/workload-launches/reconcile",
            &ReconcileLaunchesRequest {
                after: None,
                limit: 100,
            },
            Some(&fixture.credential),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let page: ReconcileLaunchesPage = serde_json::from_value(value)?;
    assert!(!page
        .items
        .iter()
        .any(|item| item.request == reserved.request));
    fixture.ctx.cleanup().await?;
    Ok(())
}
