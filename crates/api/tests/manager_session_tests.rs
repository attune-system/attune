use anyhow::Result;
use attune_api::{server::Server, state::AppState};
use attune_common::{
    config::{BlobStorageConfig, Config},
    manager_control::{
        ManagerControlClient, ManagerEnrollment, ManagerEnrollmentGrant, ManagerSession,
        MANAGER_CONTROL_PATH,
    },
    models::WorkerRole,
    repositories::manager_session::ManagerSessionRepository,
    test_database::TestDatabase,
    Error,
};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ACTION_SECRET: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const SENSOR_SECRET: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn grant(role: WorkerRole, pool: &str, secret: &str) -> ManagerEnrollmentGrant {
    ManagerEnrollmentGrant {
        role,
        pool: pool.into(),
        secret_sha256: hex::encode(Sha256::digest(secret.as_bytes())),
        labels: Default::default(),
        taints: Vec::new(),
    }
}

async fn serve(
    state: AppState,
    stop: CancellationToken,
) -> Result<(String, JoinHandle<std::io::Result<()>>)> {
    serve_router(Server::new(Arc::new(state)).router(), stop).await
}

async fn serve_router(
    app: axum::Router,
    stop: CancellationToken,
) -> Result<(String, JoinHandle<std::io::Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
    });
    Ok((base_url, server))
}

#[tokio::test]
async fn manager_sessions_are_scoped_revocable_and_retry_safe() -> Result<()> {
    attune_common::auth::crypto_provider::install();
    let config_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.test.yaml");
    let mut config = Config::load_from_file(config_path.to_str().unwrap())?;
    let scratch = tempfile::tempdir()?;
    config.storage = BlobStorageConfig::Filesystem {
        root: scratch.path().join("blobs"),
    };
    config.security.jwt_secret = Some("isolated-test-replica-signing-key-manager-control".into());
    config.manager_enrollments = vec![
        grant(WorkerRole::Action, "build", ACTION_SECRET),
        grant(WorkerRole::Sensor, "timer", SENSOR_SECRET),
    ];
    let db = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let stop = CancellationToken::new();
    let _ensure_shutdown = stop.clone().drop_guard();
    let (first_url, first_task) = serve(
        AppState::new(db.pool().clone(), config.clone()),
        stop.clone(),
    )
    .await?;
    config
        .manager_enrollments
        .retain(|grant| grant.role == WorkerRole::Sensor);
    let (restricted_url, restricted_task) = serve(
        AppState::new(db.pool().clone(), config.clone()),
        stop.clone(),
    )
    .await?;
    config
        .manager_enrollments
        .push(grant(WorkerRole::Action, "build", ACTION_SECRET));
    let (second_url, second_task) =
        serve(AppState::new(db.pool().clone(), config), stop.clone()).await?;

    let result: Result<()> = async {
        let first = ManagerControlClient::new(&first_url, true)?;
        let second = ManagerControlClient::new(&second_url, true)?;
        let restricted = ManagerControlClient::new(&restricted_url, true)?;
        let mut enrollment = ManagerEnrollment {
            role: WorkerRole::Action,
            pool: "build".into(),
            name: "runner-0".into(),
            instance: Uuid::new_v4(),
        };

        assert!(matches!(
            first.enroll(SENSOR_SECRET, &enrollment).await,
            Err(Error::PermissionDenied(_))
        ));
        let (one, two) = tokio::try_join!(
            first.enroll(ACTION_SECRET, &enrollment),
            second.enroll(ACTION_SECRET, &enrollment),
        )?;
        assert_eq!(one.id, two.id);
        assert_eq!(one.expires_at, two.expires_at);
        assert!(one.token == two.token);
        assert!(matches!(
            restricted.renew(&one).await,
            Err(Error::PermissionDenied(_))
        ));
        assert!(matches!(
            restricted.withdraw(&one).await,
            Err(Error::PermissionDenied(_))
        ));

        let impostor = ManagerEnrollment {
            instance: Uuid::new_v4(),
            ..enrollment.clone()
        };
        assert!(matches!(
            second.enroll(ACTION_SECRET, &impostor).await,
            Err(Error::InvalidState(_))
        ));
        let renewed = second.renew(&one).await?;
        assert_eq!(renewed.id, one.id);
        assert!(renewed.expires_at >= one.expires_at);
        assert_eq!(first.renew(&one).await?.id, one.id);
        let mut expired_effect = db.pool().begin().await?;
        assert!(ManagerSessionRepository::renew(
            &mut expired_effect,
            one.id,
            chrono::DateTime::UNIX_EPOCH
        )
        .await?
        .is_none());
        expired_effect.rollback().await?;

        let client = reqwest::Client::new();
        let other_api = client
            .get(format!("{first_url}/auth/me"))
            .bearer_auth(&renewed.token)
            .send()
            .await?;
        assert_eq!(other_api.status(), StatusCode::UNAUTHORIZED);
        let mut bad = renewed.token.clone();
        bad.push('a');
        assert_eq!(
            client
                .post(format!("{first_url}{MANAGER_CONTROL_PATH}/session"))
                .bearer_auth(bad)
                .send()
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );

        first.withdraw(&renewed).await?;
        second.withdraw(&renewed).await?;
        assert!(matches!(
            second.renew(&one).await,
            Err(Error::PermissionDenied(_))
        ));
        assert!(matches!(
            first.enroll(ACTION_SECRET, &enrollment).await,
            Err(Error::InvalidState(_))
        ));
        let replacement = second.enroll(ACTION_SECRET, &impostor).await?;
        assert_ne!(replacement.id, one.id);
        first.withdraw(&one).await?;
        assert_eq!(first.renew(&replacement).await?.id, replacement.id);

        // Advance the durable deadline without a wall-clock sleep.
        sqlx::query("UPDATE manager_session SET expires_at = clock_timestamp() WHERE id=$1")
            .bind(replacement.id)
            .execute(db.pool())
            .await?;
        assert!(matches!(
            second.renew(&replacement).await,
            Err(Error::PermissionDenied(_))
        ));
        assert!(matches!(
            second.withdraw(&replacement).await,
            Err(Error::PermissionDenied(_))
        ));
        let fresh = ManagerEnrollment {
            instance: Uuid::new_v4(),
            ..enrollment.clone()
        };
        assert_ne!(
            first.enroll(ACTION_SECRET, &fresh).await?.id,
            replacement.id
        );

        enrollment.role = WorkerRole::Sensor;
        enrollment.pool = "timer".into();
        assert!(matches!(
            first.enroll(ACTION_SECRET, &enrollment).await,
            Err(Error::PermissionDenied(_))
        ));
        let sensor = restricted.enroll(SENSOR_SECRET, &enrollment).await?;
        assert_eq!(first.renew(&sensor).await?.instance, enrollment.instance);
        Ok(())
    }
    .await;

    stop.cancel();
    first_task.await??;
    restricted_task.await??;
    second_task.await??;
    db.cleanup().await?;
    result
}

#[path = "../../common/tests/helpers.rs"]
mod fixtures;

#[path = "manager_sessions/pack_jobs.rs"]
mod pack_jobs;

mod work {
    use super::*;
    use attune_api::manager_completion_publication::publish_one;
    use attune_common::{
        models::{Execution, ExecutionStatus, Id, WorkerStatus, WorkerType},
        mq::{
            Connection, ExecutionCompletedPayload, ExecutionRequestedPayload, MessageEnvelope,
            MessageType, Publisher, PublisherConfig,
        },
        repositories::{
            component_lifecycle::PackProjectionIds,
            delegated_execution::DelegatedExecutionRepository,
            executable_snapshot::ExecutableSnapshotRepository,
            execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
            manager_assignment::{
                CompletionPublishKind, ManagerAssignmentRecovery,
                ManagerAssignmentRepository as Assignments, ManagerCompletionReceipt,
            },
            pack_release::{CreatePackReleaseInput, PackReleaseRepository},
            runtime::{CreateWorkerInput, WorkerRepository},
            schema_contract::{BindingOwner, SchemaContractRepository},
            FindById, Update,
        },
    };
    use serde_json::{json, Value};
    use sqlx::PgPool;

    pub(super) async fn setup() -> Result<(Config, TestDatabase, tempfile::TempDir)> {
        attune_common::auth::crypto_provider::install();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.test.yaml");
        let mut config = Config::load_from_file(path.to_str().unwrap())?;
        let scratch = tempfile::tempdir()?;
        config.storage = BlobStorageConfig::Filesystem {
            root: scratch.path().join("blobs"),
        };
        config.security.jwt_secret = Some("isolated-fixture-signing-key-manager-work".into());
        config.manager_enrollments = vec![
            grant(WorkerRole::Action, "build", ACTION_SECRET),
            grant(WorkerRole::Sensor, "timer", SENSOR_SECRET),
        ];
        let db = TestDatabase::create(&config.database)
            .await?
            .with_cleanup_on_drop();
        Ok((config, db, scratch))
    }

    async fn action(pool: &PgPool) -> Result<(Id, Id)> {
        let pack = fixtures::PackFixture::new_unique("manager_work")
            .create(pool)
            .await?;
        let runtime = fixtures::RuntimeFixture::new(None, None, "fixture-runtime")
            .create(pool)
            .await?;
        let action = fixtures::ActionFixture::new(pack.id, &pack.r#ref, "execute")
            .with_runtime(runtime.id)
            .with_out_schema(json!({"password":{"type":"string","secret":true}}))
            .create(pool)
            .await?;
        let owner = fixtures::IdentityFixture::new_unique("manager_work_owner")
            .create(pool)
            .await?;
        let mut tx = pool.begin().await?;
        let release = PackReleaseRepository::create_or_get(
            &mut tx,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: pack.r#ref,
                version: pack.version,
                digest: "a".repeat(64),
                object_key: "unused/manager-work".into(),
                provider_version: "fixture".into(),
                content_path: "/unused".into(),
                archive_size: 1,
                manifest: json!({}),
            },
        )
        .await?;
        PackReleaseRepository::activate_projected(
            &mut tx,
            pack.id,
            release.id,
            &PackProjectionIds {
                actions: vec![action.id],
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await?;
        Ok((action.id, owner.id))
    }

    pub(super) async fn bind(pool: &PgPool, name: &str, session: &ManagerSession) -> Result<Id> {
        let worker = WorkerRepository::register_incarnation(
            pool,
            CreateWorkerInput {
                name: name.into(),
                worker_type: WorkerType::Local,
                runtime: None,
                host: None,
                port: None,
                status: Some(WorkerStatus::Active),
                capabilities: Some(json!({
                    "runtimes":["fixture-runtime"], "delegated_execution_authority_v1":true
                })),
                meta: None,
            },
            WorkerRole::Action,
            session.instance,
        )
        .await?;
        let mut tx = pool.begin().await?;
        let record = ManagerSessionRepository::lock_for_work(
            &mut tx,
            session.id,
            session.instance,
            session.expires_at,
        )
        .await?
        .unwrap();
        Assignments::bind_action_worker(&mut tx, worker.id, &record).await?;
        tx.commit().await?;
        Ok(worker.id)
    }

    async fn request_execution(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        action: Id,
        owner: Id,
        refs: &[String],
    ) -> Result<Execution> {
        let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut **tx, action).await?;
        let binding = SchemaContractRepository::materialize_action(tx, &snapshot).await?;
        let execution = ExecutionRepository::create_pinned(
            &mut **tx,
            CreateExecutionInput {
                action: Some(action),
                action_ref: snapshot.executable.action.r#ref.clone(),
                executor: Some(owner),
                config: Some(json!({})),
                permission_set_refs: refs.to_vec(),
                ..Default::default()
            },
            &snapshot,
        )
        .await?;
        SchemaContractRepository::attach(tx, BindingOwner::Execution, execution.id, &binding)
            .await?;
        DelegatedExecutionRepository::select(tx, execution.id, None).await?;
        Ok(execution)
    }

    pub(super) async fn offer(
        pool: &PgPool,
        action: Id,
        owner: Id,
        worker: Id,
    ) -> Result<Execution> {
        offer_with_refs(pool, action, owner, worker, &[]).await
    }

    pub(super) async fn offer_with_refs(
        pool: &PgPool,
        action: Id,
        owner: Id,
        worker: Id,
        refs: &[String],
    ) -> Result<Execution> {
        let mut tx = pool.begin().await?;
        let execution = request_execution(&mut tx, action, owner, refs).await?;
        let scheduled = ExecutionRepository::update(
            &mut *tx,
            execution.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Scheduled),
                worker: Some(worker),
                ..Default::default()
            },
        )
        .await?;
        assert!(Assignments::offer_if_bound(&mut tx, &scheduled, 30).await?);
        tx.commit().await?;
        Ok(scheduled)
    }

    pub(super) fn enrollment(role: WorkerRole, pool: &str, name: &str) -> ManagerEnrollment {
        ManagerEnrollment {
            role,
            pool: pool.into(),
            name: name.into(),
            instance: Uuid::new_v4(),
        }
    }

    async fn expire(pool: &PgPool, assignment: Id) -> Result<()> {
        sqlx::query(
            "UPDATE manager_execution_assignment SET deadline = clock_timestamp() WHERE id = $1",
        )
        .bind(assignment)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn assignment_id(pool: &PgPool, execution: Id) -> Result<Id> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM manager_execution_assignment WHERE execution = $1 AND closed_at IS NULL",
        ).bind(execution).fetch_one(pool).await?)
    }

    #[cfg(unix)]
    fn run_disposable_action() -> Result<Value> {
        use attune_common::child_process_environment::ChildProcessEnvironment;
        let environment = ChildProcessEnvironment::from_parent(
            [
                "ATTUNE_MANAGER_ENROLLMENT_SECRET",
                "ATTUNE_API_TOKEN",
                "ATTUNE__DATABASE__URL",
                "JWT_SECRET",
                "ENCRYPTION_KEY",
            ]
            .map(|key| (key.into(), "fixture-value".into())),
            &[],
        )?;
        let mut command = std::process::Command::new("/bin/sh");
        environment.apply(&mut command);
        let output = command.arg("-eu").arg("-c").arg(r#"
            test -z "${ATTUNE_MANAGER_ENROLLMENT_SECRET+x}${ATTUNE_API_TOKEN+x}${ATTUNE__DATABASE__URL+x}${JWT_SECRET+x}${ENCRYPTION_KEY+x}"
            printf '%s' '{"password":"only-a-disposable-fixture"}'
        "#).output()?;
        assert!(output.status.success());
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    #[tokio::test]
    async fn work_is_fenced_across_api_replicas() -> Result<()> {
        let (config, db, _scratch) = setup().await?;
        let pool = db.pool();
        let stop = CancellationToken::new();
        let _ensure_shutdown = stop.clone().drop_guard();
        let state = AppState::new(pool.clone(), config.clone());
        let mut listener = attune_api::postgres_listener::spawn_postgres_listener(
            pool.clone(),
            state.broadcast_tx.clone(),
            state.log_stream_wakeups.clone(),
            state.manager_work_wakeups.clone(),
            state.manager_completion_wakeup.clone(),
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(4),
            listener.wait_until_ready(),
        )
        .await??;
        let (first, first_task) = serve(state, stop.clone()).await?;
        let (second, second_task) =
            serve(AppState::new(pool.clone(), config), stop.clone()).await?;
        let first_control = ManagerControlClient::new(&first, true)?;
        let second_control = ManagerControlClient::new(&second, true)?;
        let first = format!("{first}{MANAGER_CONTROL_PATH}/work");
        let second = format!("{second}{MANAGER_CONTROL_PATH}/work");
        let client = reqwest::Client::new();
        let result: Result<()> = async {
            let (action, owner) = action(pool).await?;
            let manager_name = "worker-a";
            let manager = first_control.enroll(ACTION_SECRET, &enrollment(WorkerRole::Action, "build", manager_name)).await?;
            let other = first_control.enroll(ACTION_SECRET, &enrollment(WorkerRole::Action, "build", "worker-b")).await?;
            let sensor = first_control.enroll(SENSOR_SECRET, &enrollment(WorkerRole::Sensor, "timer", "sensor-a")).await?;
            let worker = bind(pool, manager_name, &manager).await?;
            bind(pool, "worker-b", &other).await?;

            let waiting = tokio::time::timeout(std::time::Duration::from_secs(4), first_control.wait_work(&manager));
            let (pending, scheduled) = tokio::try_join!(async { Ok::<_, anyhow::Error>(waiting.await??.unwrap()) },
                offer(pool, action, owner, worker))?;
            let assignment = pending.assignment;
            assert_eq!(pending.execution, scheduled.id);
            assert_eq!(pending.pack_release, scheduled.pack_release.unwrap());

            let url = |base: &str, id, op: &str| format!("{base}/{id}/{op}");
            let request = Uuid::new_v4();
            let claim = |base: &str, id, session: &ManagerSession, request| {
                client.post(url(base,id,"claim")).bearer_auth(&session.token).json(&json!({"request":request})).send()
            };
            assert_eq!(claim(&first, assignment, &other, request).await?.status(), StatusCode::FORBIDDEN);
            assert_eq!(claim(&first, assignment, &sensor, request).await?.status(), StatusCode::FORBIDDEN);
            assert_eq!(claim(&first, assignment, &manager, request).await?.status(), StatusCode::CREATED);
            assert_eq!(claim(&second, assignment, &manager, request).await?.status(), StatusCode::OK);
            assert_eq!(second_control.claim_work(&manager, &pending, request).await?.assignment, assignment);
            assert_eq!(claim(&second, assignment, &manager, Uuid::new_v4()).await?.status(), StatusCode::FORBIDDEN);

            #[cfg(unix)]
            let action_result = run_disposable_action()?;
            #[cfg(not(unix))]
            let action_result = json!({"password":"only-a-disposable-fixture"});
            let protected = json!({"status":"completed","result":{"succeeded":true,"exit_code":0,
                "data": action_result }});
            let finish = |base: &str, id, session: &ManagerSession, body: &Value| {
                client.post(url(base,id,"finish")).bearer_auth(&session.token).json(body).send()
            };
            assert_eq!(finish(&second, assignment, &other, &protected).await?.status(), StatusCode::FORBIDDEN);
            assert_eq!(finish(&second, assignment, &manager, &protected).await?.status(), StatusCode::OK);
            assert_eq!(finish(&first, assignment, &manager, &protected).await?.status(), StatusCode::OK);
            assert_eq!(finish(&first, assignment, &manager, &json!({"status":"failed","result":{}})).await?.status(), StatusCode::FORBIDDEN);
            let stored = ExecutionRepository::get_by_id(pool, scheduled.id).await?;
            assert_eq!(stored.status, ExecutionStatus::Completed);
            assert!(!stored.result.unwrap().to_string().contains("only-a-disposable-fixture"));
            let protected_fields: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM execution_secret_value WHERE entity_type = $1 AND entity_id = $2",
            ).bind(attune_common::secret_values::ENTITY_EXECUTION_RESULT).bind(scheduled.id).fetch_one(pool).await?;
            assert_eq!(protected_fields, 1);
            let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM manager_execution_completion WHERE assignment = $1")
                .bind(assignment).fetch_one(pool).await?;
            assert_eq!(rows, 1);

            let running = offer(pool, action, owner, worker).await?;
            let running_assignment = assignment_id(pool, running.id).await?;
            assert_eq!(claim(&second, running_assignment, &manager, Uuid::new_v4()).await?.status(), StatusCode::CREATED);
            let unclaimed = offer(pool, action, owner, worker).await?;
            let unclaimed_assignment = assignment_id(pool, unclaimed.id).await?;
            assert_eq!(client.post(format!("{first}/drain")).bearer_auth(&manager.token).send().await?.status(), StatusCode::NO_CONTENT);
            assert_eq!(claim(&second, unclaimed_assignment, &manager, Uuid::new_v4()).await?.status(), StatusCode::FORBIDDEN);
            let lease = || client.post(url(&first, running_assignment, "lease")).bearer_auth(&manager.token).send();
            assert_eq!(lease().await?.error_for_status()?.json::<Value>().await?["state"], "running");
            assert!(!WorkerRepository::find_execution_workers(pool).await?.iter().any(|candidate| candidate.id == worker));
            let prior: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
                "UPDATE manager_execution_assignment SET deadline = clock_timestamp() + interval '15 seconds' WHERE id = $1 RETURNING deadline",
            ).bind(running_assignment).fetch_one(pool).await?;
            let watch = tokio::time::timeout(std::time::Duration::from_secs(4), first_control.watch_work(&manager, running_assignment));
            let cancel_after_renewal = async {
                tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    loop {
                        let renewed: bool = sqlx::query_scalar("SELECT deadline > $2 FROM manager_execution_assignment WHERE id = $1")
                            .bind(running_assignment).bind(prior).fetch_one(pool).await?;
                        if renewed { return Ok::<_, sqlx::Error>(()); }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                }).await??;
                Ok::<_, anyhow::Error>(ExecutionRepository::update(pool, running.id, UpdateExecutionInput {
                    status: Some(ExecutionStatus::Canceling), ..Default::default()
                }).await?)
            };
            let (control, _) = tokio::try_join!(async { Ok::<_, anyhow::Error>(watch.await??) }, cancel_after_renewal)?;
            assert!(matches!(control, attune_common::manager_control::WorkLease::Cancel));
            assert_eq!(lease().await?.error_for_status()?.json::<Value>().await?["state"], "cancel");
            expire(pool, running_assignment).await?;
            assert_eq!(finish(&second, running_assignment, &manager, &protected).await?.status(), StatusCode::FORBIDDEN);
            let mut recovery_notifications = sqlx::postgres::PgListener::connect_with(pool).await?;
            recovery_notifications
                .listen_all([attune_common::repositories::manager_assignment::MANAGER_COMPLETION_WAKEUP])
                .await?;
            assert!(matches!(Assignments::recover_expired(pool).await?, Some(ManagerAssignmentRecovery::Closed)));
            tokio::time::timeout(std::time::Duration::from_secs(5), recovery_notifications.recv()).await??;
            drop(recovery_notifications);
            assert_eq!(ExecutionRepository::get_by_id(pool, running.id).await?.status, ExecutionStatus::Cancelled);

            expire(pool, unclaimed_assignment).await?;
            assert!(matches!(Assignments::recover_expired(pool).await?, Some(ManagerAssignmentRecovery::Reschedule(id)) if id == unclaimed.id));
            let retried = ExecutionRepository::get_by_id(pool, unclaimed.id).await?;
            assert_eq!(retried.status, ExecutionStatus::Requested);
            assert_eq!(retried.executor, Some(owner));
            assert_eq!(retried.worker, None);

            first_control.withdraw(&manager).await?;
            let replacement = second_control.enroll(ACTION_SECRET, &enrollment(WorkerRole::Action, "build", manager_name)).await?;
            let replacement_worker = bind(pool, manager_name, &replacement).await?;
            assert_eq!(replacement_worker, worker);
            assert_eq!(finish(&second, assignment, &replacement, &protected).await?.status(), StatusCode::FORBIDDEN);
            assert_eq!(finish(&first, assignment, &manager, &protected).await?.status(), StatusCode::UNAUTHORIZED);
            let new_work = offer(pool, action, owner, replacement_worker).await?;
            let new_assignment = assignment_id(pool, new_work.id).await?;
            assert_eq!(claim(&first, new_assignment, &replacement, Uuid::new_v4()).await?.status(), StatusCode::CREATED);
            expire(pool, new_assignment).await?;
            assert!(matches!(Assignments::recover_expired(pool).await?, Some(ManagerAssignmentRecovery::Closed)));
            assert_eq!(ExecutionRepository::get_by_id(pool, new_work.id).await?.status, ExecutionStatus::Abandoned);
            Ok(())
        }.await;
        stop.cancel();
        listener.shutdown().await;
        first_task.await??;
        second_task.await??;
        db.cleanup().await?;
        result
    }

    async fn finish_and_wait_for_receipt(
        pool: &PgPool,
        publisher: &Publisher,
        scope: attune_common::repositories::manager_assignment::ManagerAssignmentScope,
        action: Id,
        owner: Id,
        worker: Id,
        key: &str,
    ) -> Result<()> {
        use std::time::Duration;

        let mut tx = pool.begin().await?;
        let requested = request_execution(&mut tx, action, owner, &[]).await?;
        tx.commit().await?;
        publisher
            .publish_envelope(&MessageEnvelope::new(
                MessageType::ExecutionRequested,
                ExecutionRequestedPayload {
                    execution_id: requested.id,
                    action_id: requested.action,
                    action_ref: requested.action_ref,
                    parent_id: requested.parent,
                    enforcement_id: requested.enforcement,
                    config: requested.config,
                    release_id: requested.pack_release,
                    release_digest: requested.pack_release_digest,
                },
            ))
            .await?;

        let offer = loop {
            let mut tx = pool.begin().await?;
            let offer = Assignments::next_offer(&mut tx, scope).await?;
            tx.commit().await?;
            if let Some(offer) = offer {
                break offer;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        anyhow::ensure!(offer.execution == requested.id && offer.worker == worker);
        let mut tx = pool.begin().await?;
        Assignments::claim(&mut tx, scope, offer.id, Uuid::new_v4()).await?;
        Assignments::finish(
            &mut tx,
            scope,
            offer.id,
            "service-fingerprint",
            ExecutionStatus::Completed,
            json!({"succeeded": true}),
            key,
        )
        .await?;
        tx.commit().await?;

        loop {
            let processed: bool = sqlx::query_scalar(
                "SELECT processed_at IS NOT NULL FROM manager_execution_completion WHERE assignment = $1",
            )
            .bind(offer.id)
            .fetch_one(pool)
            .await?;
            if processed {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn exercise_replay_budget(
        pool: &PgPool,
        config: &Config,
        publisher: &Arc<Publisher>,
        queue: &str,
        example_assignment: Id,
    ) -> Result<()> {
        use std::time::Duration;
        use tokio::time::{self, Instant};

        // Model a broker with a bound queue but no consumers. Seed durable
        // rows as if their confirmed deliveries had expired without receipts.
        let replay_ids: Vec<Id> = sqlx::query_scalar(
            "WITH assignments AS (
                 INSERT INTO manager_execution_assignment
                   (execution,worker,manager_session,instance,scheduled_at,pack_release,
                    pack_release_digest,deadline,closed_at)
                 SELECT execution,worker,manager_session,instance,scheduled_at,pack_release,
                        pack_release_digest,deadline,closed_at
                 FROM manager_execution_assignment CROSS JOIN generate_series(1,33)
                 WHERE id=$1 RETURNING id
             )
             INSERT INTO manager_execution_completion
               (assignment,message_id,execution,action,action_ref,status,completed_at,
                published_at,available_at)
             SELECT a.id,gen_random_uuid(),o.execution,o.action,o.action_ref,o.status,
                    o.completed_at,clock_timestamp()-interval '1 hour',
                    clock_timestamp()-interval '1 hour'
             FROM assignments a CROSS JOIN manager_execution_completion o
             WHERE o.assignment=$1 RETURNING assignment",
        )
        .bind(example_assignment)
        .fetch_all(pool)
        .await?;

        let state = Arc::new(AppState::new(pool.clone(), config.clone()));
        state.set_publisher(publisher.clone()).await;
        let mut listener = attune_api::postgres_listener::spawn_postgres_listener(
            pool.clone(),
            state.broadcast_tx.clone(),
            state.log_stream_wakeups.clone(),
            state.manager_work_wakeups.clone(),
            state.manager_completion_wakeup.clone(),
        );
        let stop = CancellationToken::new();
        let mut task = None;
        let result = time::timeout(Duration::from_secs(20), async {
            time::timeout(Duration::from_secs(5), listener.wait_until_ready()).await??;
            task = Some(tokio::spawn(
                attune_api::manager_completion_publication::run(
                    state,
                    stop.clone(),
                    time::interval_at(Instant::now(), Duration::from_secs(60)),
                ),
            ));
            let channel = publisher.channel().await;
            let mut deliveries = channel
                .basic_consume(
                    queue.into(),
                    "replay-budget".into(),
                    Default::default(),
                    Default::default(),
                )
                .await?;
            use futures::StreamExt;
            for _ in 0..32 {
                let delivery = deliveries
                    .next()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("Missing replay"))??;
                delivery.acker.ack(Default::default()).await?;
            }
            // Commit a fresh completion hint before the next replay tick.
            let fresh = Uuid::new_v4();
            let mut tx = pool.begin().await?;
            let fresh_assignment: Id = sqlx::query_scalar(
                "WITH assignment AS (
                     INSERT INTO manager_execution_assignment
                       (execution,worker,manager_session,instance,scheduled_at,pack_release,
                        pack_release_digest,deadline,closed_at)
                     SELECT execution,worker,manager_session,instance,scheduled_at,pack_release,
                            pack_release_digest,deadline,closed_at
                     FROM manager_execution_assignment WHERE id=$1 RETURNING id
                 )
                 INSERT INTO manager_execution_completion
                   (assignment,message_id,execution,action,action_ref,status,completed_at)
                 SELECT a.id,$2,o.execution,o.action,o.action_ref,o.status,o.completed_at
                 FROM assignment a CROSS JOIN manager_execution_completion o
                 WHERE o.assignment=$1 RETURNING assignment",
            )
            .bind(example_assignment)
            .bind(fresh)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query("SELECT pg_notify($1, '')")
                .bind(attune_common::repositories::manager_assignment::MANAGER_COMPLETION_WAKEUP)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            let delivery = deliveries
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("Missing first publication"))??;
            let message = MessageEnvelope::<ExecutionCompletedPayload>::from_bytes(&delivery.data)?;
            anyhow::ensure!(
                message.message_id == fresh,
                "Replay exceeded its budget or delayed a new completion"
            );
            delivery.acker.ack(Default::default()).await?;
            channel
                .basic_cancel("replay-budget".into(), Default::default())
                .await?;
            let mut fixture_ids = replay_ids;
            fixture_ids.push(fresh_assignment);
            sqlx::query("DELETE FROM manager_execution_assignment WHERE id=ANY($1)")
                .bind(fixture_ids)
                .execute(pool)
                .await?;
            Ok(())
        })
        .await;
        stop.cancel();
        if let Some(task) = task {
            time::timeout(Duration::from_secs(5), task).await??;
        }
        listener.shutdown().await;
        result?
    }

    #[tokio::test]
    #[ignore = "requires an exclusively owned disposable RabbitMQ vhost"]
    async fn completion_survives_broker_return_and_reuses_one_event_id() -> Result<()> {
        let (mut config, db, scratch) = setup().await?;
        let pool = db.pool();
        let (action, owner) = action(pool).await?;
        let request = enrollment(WorkerRole::Action, "build", "broker-test");
        let digest = grant(WorkerRole::Action, "build", ACTION_SECRET).secret_sha256;
        let session = ManagerSessionRepository::enroll(pool, &request, &digest)
            .await?
            .unwrap();
        let worker = bind(
            pool,
            &request.name,
            &ManagerSession {
                id: session.id,
                instance: session.instance,
                expires_at: session.expires_at,
                token: String::new(),
            },
        )
        .await?;
        let execution = offer(pool, action, owner, worker).await?;
        let assignment = assignment_id(pool, execution.id).await?;
        let scope = attune_common::repositories::manager_assignment::ManagerAssignmentScope {
            session: session.id,
            instance: session.instance,
            bearer_expires_at: session.expires_at,
        };
        use attune_common::repositories::manager_assignment::MANAGER_COMPLETION_WAKEUP;
        let mut listener = sqlx::postgres::PgListener::connect_with(pool).await?;
        listener.listen_all([MANAGER_COMPLETION_WAKEUP]).await?;
        let mut tx = pool.begin().await?;
        Assignments::claim(&mut tx, scope, assignment, Uuid::new_v4()).await?;
        tx.commit().await?;

        let mut tx = pool.begin().await?;
        Assignments::finish(
            &mut tx,
            scope,
            assignment,
            "test-fingerprint",
            ExecutionStatus::Completed,
            json!({"succeeded":true}),
            config.security.encryption_key.as_deref().unwrap(),
        )
        .await?;
        tx.rollback().await?;
        // A committed marker on the same channel distinguishes a suppressed
        // rollback notification without relying on a timeout for its absence.
        sqlx::query("SELECT pg_notify($1, 'rollback-marker')")
            .bind(MANAGER_COMPLETION_WAKEUP)
            .execute(pool)
            .await?;
        let receive = std::time::Duration::from_secs(5);
        assert_eq!(
            tokio::time::timeout(receive, listener.recv())
                .await??
                .payload(),
            "rollback-marker"
        );
        let mut tx = pool.begin().await?;
        Assignments::finish(
            &mut tx,
            scope,
            assignment,
            "test-fingerprint",
            ExecutionStatus::Completed,
            json!({"succeeded":true}),
            config.security.encryption_key.as_deref().unwrap(),
        )
        .await?;
        tx.commit().await?;
        assert_eq!(
            tokio::time::timeout(receive, listener.recv())
                .await??
                .payload(),
            ""
        );
        drop(listener);

        let broker_url = std::env::var("ATTUNE_MANAGER_TEST_AMQP_URL")?;
        let broker = Connection::connect(&broker_url).await?;
        let mq = attune_common::mq::MessageQueueConfig::default();
        broker.setup_common_infrastructure(&mq).await?;
        let publisher = Arc::new(
            Publisher::new(
                &broker,
                PublisherConfig {
                    confirm_publish: true,
                    timeout_secs: 5,
                    exchange: mq.rabbitmq.exchanges.executions.name.clone(),
                },
            )
            .await?,
        );
        assert!(
            publish_one(pool, &publisher, CompletionPublishKind::Initial)
                .await
                .unwrap_err()
                .to_string()
                .contains("could not be routed")
        );
        let pending: bool = sqlx::query_scalar(
            "SELECT published_at IS NULL FROM manager_execution_completion WHERE assignment = $1",
        )
        .bind(assignment)
        .fetch_one(pool)
        .await?;
        assert!(pending);
        sqlx::query("UPDATE manager_execution_completion SET available_at = clock_timestamp() WHERE assignment = $1")
            .bind(assignment).execute(pool).await?;
        broker.setup_executor_infrastructure(&mq).await?;
        assert!(publish_one(pool, &publisher, CompletionPublishKind::Replay).await?);

        let channel = publisher.channel().await;
        let queue = &mq.rabbitmq.queues.execution_completed.name;
        let first = channel
            .basic_get(queue.as_str().into(), Default::default())
            .await?
            .unwrap();
        let message =
            MessageEnvelope::<ExecutionCompletedPayload>::from_bytes(&first.delivery.data)?;
        assert_eq!(message.payload.execution_id, execution.id);
        let ManagerCompletionReceipt::Process(receipt) =
            Assignments::begin_completion_processing(pool, message.message_id, &message.payload)
                .await?
        else {
            panic!("initial completion was already processed");
        };
        assert!(matches!(
            Assignments::begin_completion_processing(pool, message.message_id, &message.payload)
                .await?,
            ManagerCompletionReceipt::Busy
        ));
        receipt.release(false).await?;
        first.delivery.acker.ack(Default::default()).await?;

        // Simulate durable state after a publisher disappeared before recording its broker ack.
        sqlx::query("UPDATE manager_execution_completion SET published_at = NULL, claim = gen_random_uuid(), available_at = clock_timestamp() WHERE assignment = $1")
            .bind(assignment).execute(pool).await?;
        assert!(publish_one(pool, &publisher, CompletionPublishKind::Replay).await?);
        let second = channel
            .basic_get(queue.as_str().into(), Default::default())
            .await?
            .unwrap();
        let repeated =
            MessageEnvelope::<ExecutionCompletedPayload>::from_bytes(&second.delivery.data)?;
        assert_eq!(message.message_id, repeated.message_id);
        let ManagerCompletionReceipt::Process(receipt) =
            Assignments::begin_completion_processing(pool, repeated.message_id, &repeated.payload)
                .await?
        else {
            panic!("unfinished completion should be replayed");
        };
        receipt.release(true).await?;
        assert!(matches!(
            Assignments::begin_completion_processing(pool, repeated.message_id, &repeated.payload)
                .await?,
            ManagerCompletionReceipt::AlreadyProcessed
        ));
        second.delivery.acker.ack(Default::default()).await?;
        assert!(!publish_one(pool, &publisher, CompletionPublishKind::Replay).await?);

        exercise_replay_budget(pool, &config, &publisher, queue, assignment).await?;

        // Exercise the normal executor consumer, placement, and completion paths.
        config.database.url = db.database_url().into();
        config.message_queue.as_mut().unwrap().url = broker_url;
        config.artifacts_dir = scratch.path().join("artifacts").display().to_string();
        config.packs_base_dir = scratch.path().join("packs").display().to_string();
        config.runtime_envs_dir = scratch.path().join("runtimes").display().to_string();
        let key = config.security.encryption_key.clone().unwrap();
        let executor = attune_executor::service::ExecutorService::new(config.clone()).await?;
        let executor_task = tokio::spawn({
            let executor = executor.clone();
            async move { executor.start().await }
        });
        let execution_path: Result<()> = async {
            for use_notification in [true, false] {
                let state = Arc::new(AppState::new(pool.clone(), config.clone()));
                state.set_publisher(publisher.clone()).await;
                let mut listener = use_notification.then(|| {
                    attune_api::postgres_listener::spawn_postgres_listener(
                        pool.clone(),
                        state.broadcast_tx.clone(),
                        state.log_stream_wakeups.clone(),
                        state.manager_work_wakeups.clone(),
                        state.manager_completion_wakeup.clone(),
                    )
                });
                let publication_result: Result<()> = async {
                    use std::time::Duration;
                    use tokio::time::{self, Instant};

                    let first_tick = if let Some(listener) = listener.as_mut() {
                        time::timeout(Duration::from_secs(5), listener.wait_until_ready())
                            .await??;
                        // Drop the initial reconciliation hint so only a real
                        // committed completion can wake the publisher here.
                        time::timeout(
                            Duration::from_secs(5),
                            state.manager_completion_wakeup.notified(),
                        )
                        .await?;
                        // The assertion deadline expires before any fallback tick.
                        Instant::now() + Duration::from_secs(60)
                    } else {
                        // Without a listener the publisher must use its fallback.
                        Instant::now()
                    };
                    let publication_stop = CancellationToken::new();
                    let _publication_guard = publication_stop.clone().drop_guard();
                    let publication_task =
                        tokio::spawn(attune_api::manager_completion_publication::run(
                            state,
                            publication_stop.clone(),
                            time::interval_at(first_tick, Duration::from_secs(1)),
                        ));
                    let work_result = time::timeout(
                        Duration::from_secs(20),
                        finish_and_wait_for_receipt(
                            pool, &publisher, scope, action, owner, worker, &key,
                        ),
                    )
                    .await;
                    publication_stop.cancel();
                    time::timeout(Duration::from_secs(5), publication_task).await??;
                    work_result??;
                    Ok(())
                }
                .await;
                if let Some(listener) = listener {
                    listener.shutdown().await;
                }
                publication_result?;
            }
            Ok(())
        }
        .await;
        executor.stop().await?;
        tokio::time::timeout(std::time::Duration::from_secs(15), executor_task).await???;
        drop(publisher);
        broker.close().await?;
        db.cleanup().await?;
        execution_path
    }
}

#[cfg(unix)]
#[path = "manager_sessions/workloads.rs"]
mod workloads;
