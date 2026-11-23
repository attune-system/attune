use super::*;
use anyhow::Context;
use attune_common::{
    blob_store::{body_from_file, hash_file, BlobStore, ObjectKey, S3BlobStore},
    child_process_environment::ChildProcessEnvironment,
    manager_control::{LogChannel, LogResume, OpenLog},
    models::{Action, ArtifactBodyState, ExecutionStatus, Id, Sensor},
    pack_registry::PackStorage,
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository},
        component_lifecycle::PackProjectionIds,
        execution::ExecutionRepository,
        log_delivery::{DeliverySegment, PrepareLogDelivery},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        rule::{CreateRuleInput, RuleRepository},
        trigger::{CreateTriggerInput, TriggerRepository},
        Create, FindById, WorkerRepository,
    },
    runtime_log_completion::FrameTotals,
    runtime_read_log,
    workload_sandbox::WorkloadSandbox,
};
use attune_worker::{
    log_delivery_spool::{LogDeliverySpool, SpoolDelivery, SpoolSegment},
    manager_pack_worker::{run_manager, ActionManagerConfig, PackJobWorkerConfig},
};
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;

struct Workloads {
    action: Action,
    owner: Id,
    sensor: Sensor,
    trigger: String,
}

fn local_environment() -> Result<ChildProcessEnvironment> {
    Ok(ChildProcessEnvironment::from_parent(
        [
            "ATTUNE_MANAGER_ENROLLMENT_FILE",
            "ATTUNE__DATABASE__URL",
            "ATTUNE__MESSAGE_QUEUE__URL",
            "JWT_SECRET",
            "ENCRYPTION_KEY",
            "AWS_SECRET_ACCESS_KEY",
        ]
        .map(|key| (key.into(), "disposable-parent-value".into())),
        &[],
    )?
    .with_workload_sandbox(WorkloadSandbox::Bubblewrap))
}

async fn seed(state: &AppState, root: &Path) -> Result<Workloads> {
    let pool = &state.db;
    let pack = fixtures::PackFixture::new("manager-live")
        .create(pool)
        .await?;
    let owner = fixtures::IdentityFixture::new_unique("manager-live-owner")
        .create(pool)
        .await?;
    let runtime = fixtures::RuntimeFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "shell")
        .with_distributions(json!({
            "verification": {"commands": [{"binary":"/bin/bash","args":["--version"]}]}
        }))
        .create(pool)
        .await?;
    let action = fixtures::ActionFixture::new(pack.id, &pack.r#ref, "echo")
        .with_runtime(runtime.id)
        .with_entrypoint("echo.sh")
        .with_out_schema(json!({"password":{"type":"string","secret":true}}))
        .create(pool)
        .await?;
    let sensor = fixtures::SensorFixture::new(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref,
        "source",
    )
    .with_entrypoint("source.sh")
    .create(pool)
    .await?;
    let trigger = TriggerRepository::create(
        pool,
        CreateTriggerInput {
            r#ref: format!("{}.sample", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Sample".into(),
            description: None,
            enabled: true,
            param_schema: None,
            event_config_schema: None,
            out_schema: None,
            sensor: Some(sensor.id),
            sensor_ref: Some(sensor.r#ref.clone()),
            is_adhoc: false,
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await?;
    let rule = RuleRepository::create(
        pool,
        CreateRuleInput {
            r#ref: format!("{}.demand", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Demand".into(),
            description: None,
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: Some(owner.id),
        },
    )
    .await?;
    let source = root.join("source");
    std::fs::create_dir_all(source.join("actions"))?;
    std::fs::create_dir_all(source.join("sensors"))?;
    std::fs::write(
        source.join("pack.yaml"),
        format!("ref: {}\nversion: {}\n", pack.r#ref, pack.version),
    )?;
    let check = r#"test -z "${ATTUNE_MANAGER_ENROLLMENT_FILE+x}${ATTUNE__DATABASE__URL+x}${ATTUNE__MESSAGE_QUEUE__URL+x}${JWT_SECRET+x}${ENCRYPTION_KEY+x}${AWS_SECRET_ACCESS_KEY+x}""#;
    std::fs::write(
        source.join("actions/echo.sh"),
        format!(
            r#"set -eu
{check}
test -z "${{ATTUNE_API_TOKEN+x}}"
cat >/dev/null
printf '%s\n' '{{"password":"disposable-protected-output"}}'
printf '%s\n' 'disposable action stderr' >&2
"#
        ),
    )?;
    std::fs::write(
        source.join("sensors/source.sh"),
        format!(
            r#"set -eu
{check}
test -n "$ATTUNE_API_TOKEN"
cat >/dev/null
curl --fail --silent --show-error -o /dev/null -X POST "$ATTUNE_API_URL/api/v1/events" \
    -H "Authorization: Bearer $ATTUNE_API_TOKEN" -H 'Content-Type: application/json' \
    -d '{{"trigger_ref":"{}","payload":{{"source":"sandboxed sensor"}}}}'
printf '%s\n' 'disposable sensor output'
while sleep 1; do :; done
"#,
            trigger.r#ref
        ),
    )?;
    let release = PackStorage::new(root.join("published")).publish_release(
        &source,
        &pack.r#ref,
        &pack.version,
    )?;
    let key = ObjectKey::new("manager-live/release.tar.gz")?;
    let (_, digest) = hash_file(&release.archive_path).await?;
    let body = state
        .blob_store
        .put(&key, body_from_file(&release.archive_path).await?, digest)
        .await?;
    let mut tx = pool.begin().await?;
    let registered = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref,
            version: pack.version,
            digest: release.digest,
            object_key: key.as_str().to_owned(),
            provider_version: body.provider_version.as_stored().to_owned(),
            content_path: release.pack_path.to_string_lossy().into_owned(),
            archive_size: body.size.try_into()?,
            manifest: serde_json::to_value(release.manifest)?,
        },
    )
    .await?;
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        registered.id,
        &PackProjectionIds {
            actions: vec![action.id],
            sensors: vec![sensor.id],
            triggers: vec![trigger.id],
            runtimes: vec![runtime.id],
            rules: vec![rule.id],
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Workloads {
        action,
        owner: owner.id,
        sensor,
        trigger: trigger.r#ref,
    })
}

async fn wait_for<T, F, Fut>(description: &str, mut predicate: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>>>,
{
    tokio::time::timeout(Duration::from_secs(32), async {
        loop {
            if let Some(item) = predicate().await? {
                return Ok(item);
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    })
    .await
    .with_context(|| format!("Timed out waiting for {description}"))?
}

async fn report_runtime(
    api: &ManagerControlClient,
    session: &ManagerSession,
    root: &Path,
) -> Result<Id> {
    let catalog = api
        .runtime_catalog(session, None)
        .await?
        .context("runtime catalog missing")?;
    // Protocol-only tests report the fixture runtime without requiring Bubblewrap.
    let report = attune_common::runtime_detection::probe_manager_catalog(
        &catalog,
        1,
        &ChildProcessEnvironment::default(),
        root,
        None,
        false,
    )
    .await?;
    Ok(api.report_runtime(session, &report).await?.worker)
}

async fn log_ready(pool: &PgPool, reference: &str) -> Result<Option<Id>> {
    Ok(sqlx::query_scalar(
        "SELECT v.id FROM artifact a JOIN artifact_version v ON v.artifact=a.id
         JOIN log_stream s ON s.artifact_version=v.id WHERE a.ref=$1 AND v.body_state='ready'
           AND s.sealed ORDER BY v.id DESC LIMIT 1",
    )
    .bind(reference)
    .fetch_optional(pool)
    .await?)
}

async fn s3_state(mut config: Config, pool: PgPool, suffix: &str) -> Result<AppState> {
    let bucket = std::env::var("ATTUNE_TEST_S3_BUCKET")?;
    let endpoint = std::env::var("ATTUNE_TEST_S3_ENDPOINT")?;
    let prefix = format!(
        "{}/{suffix}/{}",
        std::env::var("ATTUNE_TEST_S3_PREFIX")?,
        Uuid::new_v4()
    );
    config.storage = BlobStorageConfig::S3 {
        bucket: bucket.clone(),
        endpoint: Some(endpoint.clone()),
        prefix: prefix.clone(),
        region: "us-east-1".into(),
        kms_key: None,
    };
    let store = S3BlobStore::new(&bucket, "us-east-1", &prefix, Some(&endpoint), None)?;
    store.preflight().await?;
    let mut state = AppState::new(pool, config);
    state.blob_store = Arc::new(store);
    Ok(state)
}

#[tokio::test]
#[ignore = "requires Bubblewrap and the disposable versioned S3 fixture"]
async fn scoped_managers_execute_and_emit_on_private_runtime_roots() -> Result<()> {
    let (mut config, db, scratch) = work::setup().await?;
    config.message_queue = None;
    let pool = db.pool();
    let state = s3_state(config, pool.clone(), "manager-workloads").await?;
    let fixture = seed(&state, scratch.path()).await?;
    let api_stop = CancellationToken::new();
    let _api_guard = api_stop.clone().drop_guard();
    use std::sync::atomic::{AtomicUsize, Ordering};
    let finishes = Arc::new(AtomicUsize::new(0));
    let observed = finishes.clone();
    let app = Server::new(Arc::new(state))
        .router()
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = observed.clone();
                async move {
                    let path = request.uri().path();
                    if path.starts_with("/api/v1/internal/managers/work/")
                        && path.ends_with("/finish")
                    {
                        match observed.fetch_add(1, Ordering::SeqCst) {
                            0 => {
                                return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
                            }
                            1 => {
                                let committed = next.run(request).await;
                                if committed.status().is_success() {
                                    return axum::http::StatusCode::SERVICE_UNAVAILABLE
                                        .into_response();
                                }
                                return committed;
                            }
                            _ => {}
                        }
                    }
                    next.run(request).await
                }
            },
        ));
    use axum::response::IntoResponse;
    let (url, api) = serve_router(app, api_stop.clone()).await?;
    let control = ManagerControlClient::new(&url, true)?;
    let stop = CancellationToken::new();
    let _stop_guard = stop.clone().drop_guard();
    let worker = control
        .enroll(
            ACTION_SECRET,
            &work::enrollment(WorkerRole::Action, "build", "runtime"),
        )
        .await?;
    let sensor = control
        .enroll(
            SENSOR_SECRET,
            &work::enrollment(WorkerRole::Sensor, "timer", "runtime"),
        )
        .await?;
    let key = scratch.path().join("worker-log-key");
    tokio::fs::write(
        &key,
        hex::encode(attune_common::crypto::random_bytes::<32>()),
    )
    .await?;
    let worker_task = tokio::spawn(run_manager(
        control.clone(),
        worker,
        PackJobWorkerConfig {
            packs: scratch.path().join("worker/packs"),
            runtimes: scratch.path().join("worker/runtime"),
            environment: local_environment()?,
            filter: None,
        },
        Some(ActionManagerConfig {
            artifacts: scratch.path().join("worker/artifacts"),
            spool: scratch.path().join("worker/spool"),
            spool_key_file: key,
            concurrency: 2,
        }),
        stop.clone(),
    ));
    let sensor_task = tokio::spawn(attune_sensor::manager_sensor::run(
        control.clone(),
        sensor,
        attune_sensor::manager_sensor::SensorManagerConfig {
            packs: scratch.path().join("sensor/packs"),
            runtimes: scratch.path().join("sensor/runtime"),
            artifacts: scratch.path().join("sensor/artifacts"),
            environment: local_environment()?,
            notifier: None,
        },
        stop.clone(),
    ));
    let result: Result<()> = async {
        let worker = wait_for("action manager registration", || async {
            Ok(WorkerRepository::find_by_name(pool, "manager/action/build/runtime").await?)
        }).await?;
        let execution = work::offer(pool, fixture.action.id, fixture.owner, worker.id).await?;
        let completed = wait_for("action completion", || async {
            let row = ExecutionRepository::find_by_id(pool, execution.id).await?.context("execution missing")?;
            if row.status == ExecutionStatus::Completed { return Ok(Some(row)); }
            anyhow::ensure!(!matches!(row.status, ExecutionStatus::Failed | ExecutionStatus::Abandoned | ExecutionStatus::Timeout), "action settled as {:?}: {:?}", row.status, row.result);
            Ok(None)
        }).await?;
        wait_for("completion retry after the lost response", || async {
            Ok((finishes.load(Ordering::SeqCst) >= 3).then_some(()))
        }).await?;
        assert!(!format!("{:?}", completed.result).contains("disposable-protected-output"));
        assert_eq!(completed.result.as_ref().and_then(|r| r.get("exit_code")), Some(&json!(0)));
        for channel in ["stdout", "stderr"] {
            let reference = format!("{}.{channel}.log", fixture.action.r#ref);
            wait_for("verified action log", || log_ready(pool, &reference)).await?;
        }
        wait_for("sensor child event", || async {
            Ok(sqlx::query_scalar::<_, Id>("SELECT id FROM event WHERE trigger_ref=$1 AND payload->>'source'='sandboxed sensor' LIMIT 1")
                .bind(&fixture.trigger).fetch_optional(pool).await?)
        }).await?;
        Ok(())
    }.await;
    stop.cancel();
    let (worker, sensor) = tokio::join!(worker_task, sensor_task);
    let sensor_log = log_ready(pool, &format!("sensor.{}.stdout", fixture.sensor.r#ref)).await;
    api_stop.cancel();
    api.await??;
    db.cleanup().await?;
    result?;
    worker??;
    sensor??;
    assert!(sensor_log?.is_some());
    Ok(())
}

#[tokio::test]
#[ignore = "requires the disposable versioned S3 fixture"]
async fn replacement_seals_exact_log_custody_through_production_routes() -> Result<()> {
    use attune_common::artifact_transport::ArtifactFileTransport;
    let (config, db, scratch) = work::setup().await?;
    let pool = db.pool();
    let state = s3_state(config, pool.clone(), "replacement-log").await?;
    let fixture = seed(&state, scratch.path()).await?;
    let stop = CancellationToken::new();
    let _stop_guard = stop.clone().drop_guard();
    let (url, api) = serve(state, stop.clone()).await?;
    let control = ManagerControlClient::new(&url, true)?;
    let session = control
        .enroll(
            ACTION_SECRET,
            &work::enrollment(WorkerRole::Action, "build", "custody"),
        )
        .await?;
    let result: Result<()> = async {
        let runtimes = scratch.path().join("runtimes");
        tokio::fs::create_dir_all(&runtimes).await?;
        let worker = report_runtime(&control, &session, &runtimes).await?;
        work::offer_with_refs(
            pool,
            fixture.action.id,
            fixture.owner,
            worker,
            &["standard".into()],
        )
        .await?;
        let offer = control
            .wait_work(&session)
            .await?
            .context("missing assignment")?;
        let request = Uuid::new_v4();
        control.claim_work(&session, &offer, request).await?;
        let launch = control.launch_action(&session, &offer, request).await?;
        let child = launch
            .child_token
            .context("opt-in child credential missing")?;
        let client = reqwest::Client::new();
        let keys = || {
            client
                .get(format!("{url}/api/v1/keys"))
                .bearer_auth(&child)
                .send()
        };
        assert_eq!(keys().await?.status(), StatusCode::OK);
        assert!(control
            .launch_action(&session, &offer, Uuid::new_v4())
            .await
            .is_err());
        let body = |channel| OpenLog { request, channel };
        let stdout = control
            .open_action_log(&session, offer.assignment, &body(LogChannel::Stdout))
            .await?;
        let retry = control
            .open_action_log(&session, offer.assignment, &body(LogChannel::Stdout))
            .await?;
        assert_eq!(stdout.version, retry.version);
        assert!(stdout.key == retry.key);
        let stderr = control
            .open_action_log(&session, offer.assignment, &body(LogChannel::Stderr))
            .await?;
        assert!(stdout.key != stderr.key);
        let plain = b"disposable protected log";
        let bytes =
            runtime_read_log::encode(&stdout.key, offer.execution, stdout.version, 0, plain)?;
        let mut totals = FrameTotals::default();
        totals.record(&bytes, plain.len())?;
        let manifest = totals
            .manifest(offer.execution, stdout.version)?
            .authenticate(&stdout.key)?;
        let transport = control.artifact_transport(&stdout.capture_token, "");
        assert!(transport
            .commit_log_segment(stderr.version, 0, &bytes)
            .await
            .is_err());
        assert!(transport
            .seal_log_stream_with_manifest(stdout.version, false, &manifest)
            .await
            .is_err());
        transport
            .commit_log_segment(stdout.version, 0, &bytes)
            .await?;
        let key = hex::encode(attune_common::crypto::random_bytes::<32>());
        let root = scratch.path().join("spool");
        let spool = || LogDeliverySpool::open(root.clone(), worker, &key, 1024 * 1024, 65536, 4096);
        let capture = spool().await?;
        let entry = SpoolDelivery {
            execution: offer.execution,
            version: stdout.version,
            stream: stdout.stream,
            worker,
            attempt: stdout.attempt,
            source_instance: session.instance,
            custody: Uuid::new_v4(),
            finalize_by: chrono::Utc::now() + chrono::Duration::seconds(55),
            truncated: false,
            authenticated_manifest: Some(manifest.clone()),
            segments: vec![SpoolSegment {
                sequence: 0,
                bytes: bytes.clone(),
            }],
        };
        capture.save(entry.clone()).await?;
        let digest = hex::encode(Sha256::digest(&bytes));
        let input = PrepareLogDelivery {
            execution: offer.execution,
            version: stdout.version,
            stream: stdout.stream,
            worker,
            instance: session.instance,
            custody: entry.custody,
            segments: vec![DeliverySegment {
                sequence: 0,
                size_bytes: bytes.len() as i64,
                sha256: digest.clone(),
            }],
            total_bytes: bytes.len() as i64,
            sha256: digest,
            truncated: false,
            authenticated_manifest: Some(manifest),
            finalize_by: entry.finalize_by,
        };
        control
            .prepare_action_log(&session, stdout.version, &input)
            .await?;
        control
            .prepare_action_log(&session, stdout.version, &input)
            .await?;
        assert!(matches!(
            control
                .resume_log(&session, stdout.version, entry.custody)
                .await?,
            LogResume::Waiting
        ));
        control
            .finish_work(
                &session,
                offer.assignment,
                &attune_common::manager_control::FinishWork {
                    status: ExecutionStatus::Completed,
                    result: Default::default(),
                },
            )
            .await?;
        assert!(matches!(
            keys().await?.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ));
        let LogResume::Upload { token: old } = control
            .resume_log(&session, stdout.version, entry.custody)
            .await?
        else {
            anyhow::bail!("terminal publication did not activate custody");
        };
        assert!(transport
            .commit_log_segment(stdout.version, 0, &bytes)
            .await
            .is_err());
        control.withdraw(&session).await?;
        let stale = control.artifact_transport(&old, "");
        assert!(stale
            .commit_log_segment(stdout.version, 0, &bytes)
            .await
            .is_err());
        drop(capture);
        let replacement = control
            .enroll(
                ACTION_SECRET,
                &work::enrollment(WorkerRole::Action, "build", "custody"),
            )
            .await?;
        assert_eq!(
            report_runtime(&control, &replacement, &runtimes).await?,
            worker
        );
        let other = control
            .enroll(
                ACTION_SECRET,
                &work::enrollment(WorkerRole::Action, "build", "unrelated"),
            )
            .await?;
        report_runtime(&control, &other, &runtimes).await?;
        assert!(control
            .resume_log(&other, stdout.version, entry.custody)
            .await
            .is_err());
        assert!(control
            .resume_log(&replacement, stdout.version, Uuid::new_v4())
            .await
            .is_err());
        let capture = spool().await?;
        let recovered = capture.load(stdout.version).await?;
        let LogResume::Upload { token } = control
            .resume_log(&replacement, recovered.version, recovered.custody)
            .await?
        else {
            anyhow::bail!("replacement could not resume custody");
        };
        let upload = control.artifact_transport(&token, "");
        assert!(upload
            .commit_log_segment(recovered.version, 0, b"different bytes")
            .await
            .is_err());
        upload
            .commit_log_segment(recovered.version, 0, &recovered.segments[0].bytes)
            .await?;
        upload
            .seal_log_stream_with_manifest(
                recovered.version,
                recovered.truncated,
                recovered.authenticated_manifest.as_deref().unwrap(),
            )
            .await?;
        assert!(matches!(
            control
                .resume_log(&replacement, recovered.version, recovered.custody)
                .await?,
            LogResume::Closed
        ));
        capture.remove(recovered.version, recovered.custody).await?;
        assert!(capture.versions().await?.is_empty());
        let version = ArtifactVersionRepository::find_by_id(pool, recovered.version)
            .await?
            .context("log version missing")?;
        assert_eq!(version.body_state, Some(ArtifactBodyState::Ready));
        assert_eq!(
            ArtifactRepository::find_by_id(pool, version.artifact)
                .await?
                .unwrap()
                .owner,
            fixture.action.r#ref
        );
        Ok(())
    }
    .await;
    stop.cancel();
    api.await??;
    db.cleanup().await?;
    result
}

#[tokio::test]
async fn sensor_start_and_disclosure_are_fenced_by_live_generation() -> Result<()> {
    let (mut config, db, scratch) = work::setup().await?;
    config.message_queue = None;
    let pool = db.pool();
    let state = AppState::new(pool.clone(), config);
    let fixture = seed(&state, scratch.path()).await?;
    let stop = CancellationToken::new();
    let _stop_guard = stop.clone().drop_guard();
    let (url, api) = serve(state, stop.clone()).await?;
    let control = ManagerControlClient::new(&url, true)?;
    let result: Result<()> = async {
        let root = scratch.path().join("runtime");
        tokio::fs::create_dir(&root).await?;
        let action = control
            .enroll(
                ACTION_SECRET,
                &work::enrollment(WorkerRole::Action, "build", "action"),
            )
            .await?;
        let sensor = control
            .enroll(
                SENSOR_SECRET,
                &work::enrollment(WorkerRole::Sensor, "timer", "generation"),
            )
            .await?;
        assert!(control.available_sensors(&action).await.is_err());
        assert!(control
            .start_sensor(&action, fixture.sensor.id, Uuid::new_v4())
            .await
            .is_err());
        let worker = report_runtime(&control, &sensor, &root).await?;
        assert!(control
            .available_sensors(&sensor)
            .await?
            .contains(&fixture.sensor.id));
        let request = Uuid::new_v4();
        let (first, replay) = tokio::try_join!(
            control.start_sensor(&sensor, fixture.sensor.id, request),
            control.start_sensor(&sensor, fixture.sensor.id, request),
        )?;
        assert_eq!(first.fence, replay.fence);
        assert!(control
            .start_sensor(&sensor, fixture.sensor.id, Uuid::new_v4())
            .await
            .is_err());
        let client = reqwest::Client::new();
        let event = |token: String, trigger: String| {
            let client = client.clone();
            let url = url.clone();
            async move {
                client
                    .post(format!("{url}/api/v1/events"))
                    .bearer_auth(token)
                    .json(&json!({"trigger_ref":trigger,"payload":{"from":"fixture"}}))
                    .send()
                    .await
            }
        };
        assert_eq!(
            event(first.token.clone(), fixture.trigger.clone())
                .await?
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            event(first.token.clone(), "unrelated.trigger".into())
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
        control.release_sensor(&sensor, first.fence).await?;
        assert!(control
            .start_sensor(&sensor, fixture.sensor.id, request)
            .await
            .is_err());
        let next = control
            .start_sensor(&sensor, fixture.sensor.id, Uuid::new_v4())
            .await?;
        assert!(next.fence.generation > first.fence.generation);
        control.withdraw(&sensor).await?;
        assert!(matches!(
            event(next.token, fixture.trigger.clone()).await?.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ));
        let replacement = control
            .enroll(
                SENSOR_SECRET,
                &work::enrollment(WorkerRole::Sensor, "timer", "generation"),
            )
            .await?;
        assert_eq!(report_runtime(&control, &replacement, &root).await?, worker);
        let started = control
            .start_sensor(&replacement, fixture.sensor.id, Uuid::new_v4())
            .await?;
        assert!(started.fence.generation > next.fence.generation);
        assert!(control
            .renew_sensor(&replacement, next.fence)
            .await
            .is_err());
        assert_eq!(
            event(started.token, fixture.trigger.clone())
                .await?
                .status(),
            StatusCode::CREATED
        );
        control.release_sensor(&replacement, started.fence).await?;
        Ok(())
    }
    .await;
    stop.cancel();
    api.await??;
    db.cleanup().await?;
    result
}
