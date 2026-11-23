use super::*;
use anyhow::Context;
use attune_common::{
    child_process_environment::ChildProcessEnvironment,
    manager_control::{PackTransferScope, RuntimeProbe, PACK_JOB_CLAIM_HEADER},
    models::{Id, PackInstall, Sensor},
    repositories::{
        manager_runtime::ManagerRuntimeRepository,
        runtime::{RuntimeRepository, UpdateRuntimeInput, WorkerRepository},
        FindById, Update,
    },
    runtime_detection::probe_manager_catalog,
    runtime_lifecycle::{PreparationScope, RuntimeRecipe},
    workload_sandbox::WorkloadSandbox,
};
use serde_json::json;
use std::time::Duration;

fn enrollment(role: WorkerRole, name: &str) -> ManagerEnrollment {
    ManagerEnrollment {
        role,
        name: name.into(),
        pool: if role == WorkerRole::Action {
            "build"
        } else {
            "timer"
        }
        .into(),
        instance: Uuid::new_v4(),
    }
}

fn recipe(version: &str) -> Result<RuntimeRecipe> {
    Ok(serde_json::from_value(json!({
        "prepare_at_install":true,
        "tools":[{"name":"shell","binaries":["/bin/sh"],"args":["-c",format!("printf {version}")],
                  "version_pattern":"(?<version>[0-9]+\\.[0-9]+)","primary":true}],
        "prepare":[{"command":["{tool_shell}","-c","printf prepared > \"$1\"","sh","{output_dir}/built"]}],
        "launch":{"command":["{tool_shell}","{source_file}","{output_dir}/built"]}
    }))?)
}

fn local_environment(digests: &str) -> Result<ChildProcessEnvironment> {
    Ok(ChildProcessEnvironment::from_parent(
        [
            ("ATTUNE_RUNTIME_RECIPE_DIGESTS".into(), digests.into()),
            (
                "ATTUNE_RUNTIME_PREPARATION_REVISION".into(),
                "manager-test-image".into(),
            ),
            (
                "ATTUNE_MANAGER_ENROLLMENT_FILE".into(),
                "/run/secrets/manager".into(),
            ),
            ("ENCRYPTION_KEY".into(), "fixture-platform-secret".into()),
        ],
        &[],
    )?
    .with_workload_sandbox(WorkloadSandbox::Bubblewrap))
}

#[tokio::test]
async fn manager_runtime_report_is_ordered_and_gates_placement() -> Result<()> {
    let (mut config, db, scratch) = work::setup().await?;
    let pool = db.pool();
    config.manager_enrollments[0]
        .labels
        .insert("site".into(), "fixture".into());
    let first_recipe = recipe("1.0")?;
    let second_recipe = recipe("2.0")?;
    let mut fixture = fixtures::RuntimeFixture::new(None, None, "manager-lifecycle");
    fixture.execution_config = json!({"lifecycle": first_recipe});
    let runtime = fixture.create(pool).await?;
    let stop = CancellationToken::new();
    let _guard = stop.clone().drop_guard();
    let (url, serving) = serve(AppState::new(pool.clone(), config), stop.clone()).await?;
    let outcome: Result<()> = async {
        let api = ManagerControlClient::new(&url, true)?;
        let slot = enrollment(WorkerRole::Action, "catalog");
        let session = api.enroll(ACTION_SECRET, &slot).await?;
        assert!(WorkerRepository::find_pack_workers(pool, true)
            .await?
            .is_empty());
        let catalog = api
            .runtime_catalog(&session, None)
            .await?
            .context("missing catalog")?;
        assert!(api
            .runtime_catalog(&session, Some(catalog.revision))
            .await?
            .is_none());
        let filter = ["MANAGER-LIFECYCLE".into()];
        let environment = local_environment(&first_recipe.digest()?)?
            .with_workload_sandbox(WorkloadSandbox::Disabled);
        let mut report = probe_manager_catalog(
            &catalog,
            2,
            &environment,
            scratch.path(),
            Some(&filter),
            true,
        )
        .await?;
        assert_eq!(
            report
                .runtimes
                .get(&runtime.id)
                .and_then(|probe| probe.primary_version.as_deref()),
            Some("1.0")
        );
        let receipt = api.report_runtime(&session, &report).await?;
        assert_eq!(
            api.report_runtime(&session, &report).await?.worker,
            receipt.worker
        );
        let row = WorkerRepository::get_by_id(pool, receipt.worker).await?;
        let capabilities = row
            .capabilities
            .as_ref()
            .context("no projected capabilities")?;
        assert_eq!(
            capabilities["labels"],
            json!({"site": "fixture", "managed_pool": "build"})
        );
        assert_eq!(
            capabilities["runtime_toolchains"][&runtime.r#ref]["versions"],
            json!(["1.0"])
        );
        assert!(WorkerRepository::find_execution_workers(pool)
            .await?
            .is_empty());
        assert_eq!(
            WorkerRepository::find_pack_workers(pool, true).await?[0].id,
            receipt.worker
        );
        report.sequence = 1;
        assert!(matches!(
            api.report_runtime(&session, &report).await,
            Err(Error::InvalidState(_))
        ));
        report.sequence = 3;
        report.runtimes.get_mut(&runtime.id).unwrap().recipe_digest = Some(second_recipe.digest()?);
        assert!(api.report_runtime(&session, &report).await.is_err());

        let mut projection = pool.begin().await?;
        RuntimeRepository::update(
            &mut *projection,
            runtime.id,
            UpdateRuntimeInput {
                execution_config: Some(json!({"lifecycle": second_recipe})),
                ..Default::default()
            },
        )
        .await?;
        // A pack plans preparation against its staged definition before commit.
        assert_eq!(
            WorkerRepository::find_pack_workers(&mut *projection, true).await?[0].id,
            receipt.worker
        );
        projection.rollback().await?;
        assert!(api
            .runtime_catalog(&session, Some(catalog.revision))
            .await?
            .is_none());
        RuntimeRepository::update(
            pool,
            runtime.id,
            UpdateRuntimeInput {
                execution_config: Some(json!({"lifecycle": second_recipe})),
                ..Default::default()
            },
        )
        .await?;
        assert!(WorkerRepository::find_pack_workers(pool, true)
            .await?
            .is_empty());
        let mut tx = pool.begin().await?;
        assert!(!ManagerRuntimeRepository::retain_placement(&mut tx, &row).await?);
        tx.rollback().await?;
        assert!(matches!(
            api.report_runtime(&session, &report).await,
            Err(Error::InvalidState(_))
        ));
        api.withdraw(&session).await?;
        let replacement = api
            .enroll(
                ACTION_SECRET,
                &ManagerEnrollment {
                    instance: Uuid::new_v4(),
                    ..slot
                },
            )
            .await?;
        assert!(api.runtime_catalog(&session, None).await.is_err());
        let current = api
            .runtime_catalog(&replacement, Some(catalog.revision))
            .await?
            .context("catalog revision did not advance")?;
        let mut report = probe_manager_catalog(
            &current,
            1,
            &local_environment(&second_recipe.digest()?)?
                .with_workload_sandbox(WorkloadSandbox::Disabled),
            scratch.path(),
            Some(&filter),
            true,
        )
        .await?;
        assert_eq!(
            report
                .runtimes
                .get(&runtime.id)
                .and_then(|probe| probe.primary_version.as_deref()),
            Some("2.0")
        );
        assert_eq!(
            api.report_runtime(&replacement, &report).await?.worker,
            receipt.worker
        );
        // A newer sequence must still resolve IDs against the active catalog.
        report.sequence += 1;
        report.runtimes.insert(
            i64::MAX,
            RuntimeProbe {
                default_available: true,
                recipe_digest: None,
                primary_version: None,
                versions: Default::default(),
            },
        );
        assert!(api.report_runtime(&replacement, &report).await.is_err());
        Ok(())
    }
    .await;
    stop.cancel();
    serving.await??;
    db.cleanup().await?;
    outcome
}

use attune_common::{
    blob_store::{
        body_from_file, hash_file, BlobBody, BlobReader, BlobStore, BlobStoreError, ByteRange,
        ObjectKey, ProviderVersion, StoredObject,
    },
    mq::{
        Connection, Consumer, ConsumerConfig, MessageEnvelope, MessageQueueConfig, MessageType,
        PackTestRequestedPayload, Publisher, PublisherConfig,
    },
    pack_registry::PackStorage,
    pack_transport::PackFileTransport,
    repositories::{
        component_lifecycle::PackProjectionIds,
        pack_release::CreatePackReleaseInput,
        runtime_preparation::{PreparationJob, RuntimePreparationRepository as Preparations},
        schema_type::SchemaTypeRepository,
        sensor_schema::SensorSchemaRepository,
        sensor_workload::{AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome},
        PackInstallRepository, PackReleaseRepository, SensorWorkloadRepository,
    },
    runtime_lifecycle::{PreparationTarget, PreparationWorkload, PREPARATION_TRIGGER},
};
use attune_worker::manager_pack_worker::{self, PackJobWorkerConfig};
use sqlx::PgPool;
use std::process::Command;
use tokio::sync::{watch, Notify};

struct PausePreparedPut {
    inner: Arc<dyn BlobStore>,
    staged: Notify,
    publish: Notify,
    pause: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl BlobStore for PausePreparedPut {
    async fn preflight(&self) -> Result<(), BlobStoreError> {
        self.inner.preflight().await
    }

    async fn put(
        &self,
        key: &ObjectKey,
        body: BlobBody,
        digest: [u8; 32],
    ) -> Result<StoredObject, BlobStoreError> {
        let stored = self.inner.put(key, body, digest).await?;
        if key.as_str().starts_with("packs/prepared/")
            && self.pause.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.staged.notify_one();
            self.publish.notified().await;
        }
        Ok(stored)
    }

    async fn get(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError> {
        self.inner.get(key, version, range).await
    }

    async fn get_pinned(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        size: u64,
        digest: [u8; 32],
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError> {
        self.inner
            .get_pinned(key, version, size, digest, range)
            .await
    }

    async fn head(&self, key: &ObjectKey) -> Result<Option<StoredObject>, BlobStoreError> {
        self.inner.head(key).await
    }

    async fn delete(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
    ) -> Result<(), BlobStoreError> {
        self.inner.delete(key, version).await
    }
}

async fn report(
    api: &ManagerControlClient,
    session: &ManagerSession,
    env: &ChildProcessEnvironment,
    root: &Path,
    pack_only: bool,
) -> Result<Id> {
    let catalog = api
        .runtime_catalog(session, None)
        .await?
        .context("No runtime catalog")?;
    let evidence = probe_manager_catalog(&catalog, 1, env, root, None, pack_only).await?;
    Ok(api.report_runtime(session, &evidence).await?.worker)
}

fn payload(install: &PackInstall, effects: bool) -> PackTestRequestedPayload {
    PackTestRequestedPayload {
        pack_install_id: install.id,
        pack_ref: install.pack_ref.clone(),
        pack_version: install.pack_version.clone(),
        candidate_path: Some("only-visible-to-api".into()),
        candidate_access_token: None,
        trigger_reason: install.trigger_reason.clone(),
        required_runtimes: vec![],
        required_recipe_digests: vec![],
        candidate_api_effects: effects,
        test_min_pass_rate: None,
        test_allows_failure: false,
        worker_selector: json!({"managed_pool":"build"}),
        worker_tolerations: json!([]),
        worker_affinity: json!({}),
    }
}

async fn wait_for_status(pool: &PgPool, install: Id, status: &str) -> Result<PackInstall> {
    let repo = PackInstallRepository::new(pool.clone());
    let started = tokio::time::Instant::now();
    loop {
        let row = repo
            .find_by_id(install)
            .await?
            .context("Pack install missing")?;
        if row.status == status {
            return Ok(row);
        }
        anyhow::ensure!(
            !attune_common::repositories::pack_install_is_terminal(&row.status)
                && started.elapsed() < Duration::from_secs(25),
            "install {install} expected {status}, got {} ({:?})",
            row.status,
            row.error_message,
        );
        tokio::time::sleep(Duration::from_millis(35)).await;
    }
}

async fn fixture_release(
    state: &AppState,
    root: &Path,
    recipe: &RuntimeRecipe,
) -> Result<(attune_common::models::PackRelease, Sensor, StoredObject)> {
    let pack = fixtures::PackFixture::new("manager-pack-jobs")
        .create(&state.db)
        .await?;
    let mut runtime =
        fixtures::RuntimeFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "shell");
    runtime.execution_config = json!({"lifecycle":recipe});
    let runtime = runtime.create(&state.db).await?;
    let sensor = fixtures::SensorFixture::new(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref,
        "fixture",
    )
    .with_entrypoint("sensors/run.sh")
    .create(&state.db)
    .await?;
    let source = root.join("candidate");
    std::fs::create_dir_all(source.join("sensors"))?;
    std::fs::write(source.join("pack.yaml"), nominal_pack_yaml(&pack.version))?;
    std::fs::write(
        source.join("test.sh"),
        r#"#!/bin/sh
set -eu
test -z "${ATTUNE_MANAGER_ENROLLMENT_FILE:-}"
test -z "${ENCRYPTION_KEY:-}"
test -n "${ATTUNE_CANDIDATE_EFFECTS_TOKEN:-}"
printf 'Total Tests: 1\nPassed: 1\nFailed: 0\n'
head -c 1048576 /dev/zero | tr '\000' x
head -c 1048576 /dev/zero | tr '\000' y >&2
"#,
    )?;
    std::fs::write(source.join("sensors/run.sh"), "#!/bin/sh\ncat \"$1\"\n")?;
    let release = PackStorage::new(root.join("published")).publish_release(
        &source,
        &pack.r#ref,
        &pack.version,
    )?;
    let key = ObjectKey::new("manager-test/candidate.tar.gz")?;
    let (_, digest) = hash_file(&release.archive_path).await?;
    let stored = state
        .blob_store
        .put(&key, body_from_file(&release.archive_path).await?, digest)
        .await?;
    assert!(stored.provider_version.as_stored().starts_with("v:"));
    let mut tx = state.db.begin().await?;
    let registered = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref,
            version: pack.version,
            digest: release.digest,
            object_key: key.as_str().into(),
            provider_version: stored.provider_version.as_stored().into(),
            content_path: release.pack_path.display().to_string(),
            archive_size: stored.size.try_into()?,
            manifest: serde_json::to_value(release.manifest)?,
        },
    )
    .await?;
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        registered.id,
        &PackProjectionIds {
            sensors: vec![sensor.id],
            runtimes: vec![runtime.id],
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok((registered, sensor, stored))
}

fn nominal_pack_yaml(version: &str) -> String {
    format!("ref: manager-pack-jobs\nversion: {version}\ntesting:\n  enabled: true\n  candidate_api_effects: true\n  discovery: {{method: explicit}}\n  runners:\n    smoke: {{type: script, entry_point: test.sh, timeout: 10}}\n")
}

#[tokio::test]
#[ignore = "requires an exclusively owned RabbitMQ vhost, versioned S3, and Bubblewrap"]
async fn manager_pack_jobs_recover_and_stream_with_exact_claim_and_generation() -> Result<()> {
    let (mut config, db, scratch) = work::setup().await?;
    let pool = db.pool();
    config.storage = BlobStorageConfig::S3 {
        bucket: std::env::var("ATTUNE_TEST_S3_BUCKET")?,
        endpoint: Some(std::env::var("ATTUNE_TEST_S3_ENDPOINT")?),
        prefix: format!(
            "{}/{}",
            std::env::var("ATTUNE_TEST_S3_PREFIX")?,
            Uuid::new_v4()
        ),
        region: "us-east-1".into(),
        kms_key: None,
    };
    let mut state = AppState::new(pool.clone(), config.clone());
    let paused = Arc::new(PausePreparedPut {
        inner: state.blob_store.clone(),
        staged: Notify::new(),
        publish: Notify::new(),
        pause: true.into(),
    });
    state.blob_store = paused.clone();
    paused.preflight().await?;
    let build_recipe = recipe("1.0")?;
    let (release, sensor, archive) = fixture_release(&state, scratch.path(), &build_recipe).await?;
    let runtime_root = scratch.path().join("runtimes");
    let env = local_environment(&build_recipe.digest()?)?;
    let prep_env = env.clone().with_object_preparation(true);
    let scope =
        PreparationScope::open(&runtime_root, &prep_env)?.context("No preparation scope")?;
    let job = PreparationJob {
        target: PreparationTarget {
            kind: PreparationWorkload::Sensor,
            reference: sensor.r#ref.clone(),
            source: "sensors/run.sh".into(),
            runtime_ref: "manager-pack-jobs.shell".into(),
            runtime_names: vec!["shell".into()],
            recipe_digest: build_recipe.digest()?,
        },
        recipe: build_recipe,
        scopes: vec![scope.clone()],
        placements: vec![],
    };
    let mq = MessageQueueConfig::default();
    let broker = Connection::connect(&std::env::var("ATTUNE_MANAGER_TEST_AMQP_URL")?).await?;
    broker.setup_common_infrastructure(&mq).await?;
    broker.setup_executor_infrastructure(&mq).await?;
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
    let consumer = Arc::new(
        Consumer::new(
            &broker,
            ConsumerConfig {
                queue: mq.rabbitmq.queues.pack_tests.name.clone(),
                tag: "manager-pack-acceptance".into(),
                prefetch_count: 4,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?,
    );
    let stop = CancellationToken::new();
    let _guard = stop.clone().drop_guard();
    let stop_processor = stop.clone();
    let processor = attune_executor::pack_test_processor::PackTestProcessor::new(
        pool.clone(),
        publisher.clone(),
        consumer,
    );
    let processor = tokio::spawn(async move {
        tokio::select! {
            result=processor.start()=>result,
            _=stop_processor.cancelled()=>Ok(()),
        }
    });
    let encryption = config.security.encryption_key.take();
    let (url, server) = serve(state, stop.clone()).await?;
    let api = ManagerControlClient::new(&url, true)?;
    let http = reqwest::Client::new();
    let workers = tokio_util::task::TaskTracker::new();
    let result:Result<()>=async {
        let repo=PackInstallRepository::new(pool.clone());
        let candidate=repo.create(&release.pack_ref,&release.version,"update",Some(release.pack),None).await?;
        let activation=repo.create(&release.pack_ref,&release.version,"update",Some(release.pack),None).await?;
        repo.begin_activation(activation.id).await?.context("Activation was not pending")?;
        let preparation=repo.create(&release.pack_ref,&release.version,PREPARATION_TRIGGER,None,None).await?;
        let mut tx=pool.begin().await?;
        for install in [&candidate,&preparation] {
            assert!(PackInstallRepository::attach_candidate_object(&mut tx,install.id,&archive).await?);
        }
        Preparations::create_attempt(&mut tx,preparation.id,activation.id,&release.digest,&job).await?;
        tx.commit().await?;
        let builder_slot=enrollment(WorkerRole::Action,"pack-builder");
        let old=Arc::new(api.enroll(ACTION_SECRET,&builder_slot).await?);
        let old_worker=report(&api,&old,&env,&runtime_root,true).await?;
        let request=|install:&PackInstall,effects| MessageEnvelope::new(MessageType::PackTestRequested,payload(install,effects));
        publisher.publish_envelope(&request(&candidate,true)).await?;
        let offer=api.wait_pack_job(&old).await?.context("Executor did not offer candidate tests")?;
        let queued: bool = sqlx::query_scalar(
            "SELECT deadline > clock_timestamp() + interval '40 seconds' FROM manager_pack_job WHERE install=$1",
        ).bind(candidate.id).fetch_one(pool).await?;
        assert!(queued);
        let candidate_claim=Uuid::new_v4();
        let claim=api.claim_pack_job(&old,&offer,candidate_claim).await?;
        let effects=claim.effects.context("Candidate effects were not issued")?;
        let replay=api.claim_pack_job(&old,&offer,candidate_claim).await?.effects.context("No effect receipt replay")?;
        assert_eq!(replay.id,effects.id);
        assert!(replay.token==effects.token);
        let effect_url=format!("{url}/api/v1/candidates/{}/effects",effects.id);
        let list=||http.post(&effect_url).header("x-attune-candidate-effects",&effects.token).json(&json!({"operation":"list_events"})).send();
        assert_eq!(list().await?.status(),StatusCode::OK);

        publisher.publish_envelope(&request(&preparation,false)).await?;
        let offer=api.wait_pack_job(&old).await?.context("Executor did not offer preparation")?;
        assert_eq!(offer.install,preparation.id);
        let prepare_claim=Uuid::new_v4();
        let claimed=api.claim_pack_job(&old,&offer,prepare_claim).await?;
        assert!(claimed.effects.is_none());
        let (_,old_session)=watch::channel(old.clone());
        let cache=scratch.path().join("old-cache");
        let transfer=api.pack_transport(old_session,PackTransferScope::PackJob {install:preparation.id,claim:prepare_claim}, cache.to_str().unwrap());
        let files=transfer.sync_claimed_candidate(&release.pack_ref,preparation.id,&release.digest).await?;
        let ready=job.recipe.prepare_for_install(&files,job.target.clone(),&release.digest,&scope,&prep_env,&CancellationToken::new()).await?;
        let upload=transfer.publish_prepared_runtime(preparation.id,"",&ready);
        let revoke=async {
            let result: Result<()>=async {
                tokio::time::timeout(Duration::from_secs(15),paused.staged.notified()).await?;
                api.withdraw(&old).await?;
                Ok(())
            }.await;
            paused.publish.notify_one();
            result
        };
        let (upload,revoked)=tokio::join!(upload,revoke);
        revoked?;
        assert!(upload.is_err());
        let discarded: bool = sqlx::query_scalar("SELECT prepared IS NULL FROM pack_runtime_preparation_attempt WHERE install=$1")
            .bind(preparation.id).fetch_one(pool).await?;
        assert!(discarded);
        assert_eq!(list().await?.status(),StatusCode::FORBIDDEN);
        let open_http=http.get(format!("{url}/api/v1/internal/pack-installs/{}/archive",preparation.id)).bearer_auth(&old.token)
            .header(attune_common::manager_control::PACK_JOB_HEADER,preparation.id.to_string())
            .header(PACK_JOB_CLAIM_HEADER,prepare_claim.to_string()).send().await?;
        assert!(matches!(open_http.status(),StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN));

        let replacement_slot=ManagerEnrollment {instance:Uuid::new_v4(),..builder_slot};
        let replacement=api.enroll(ACTION_SECRET,&replacement_slot).await?;
        let wrong=http.get(format!("{url}/api/v1/internal/pack-installs/{}/archive",preparation.id)).bearer_auth(&replacement.token)
            .header(attune_common::manager_control::PACK_JOB_HEADER,preparation.id.to_string())
            .header(PACK_JOB_CLAIM_HEADER,prepare_claim.to_string()).send().await?;
        assert_eq!(wrong.status(),StatusCode::FORBIDDEN);
        let worker_stop=stop.child_token();
        let task=workers.spawn(manager_pack_worker::run(api.clone(),replacement,PackJobWorkerConfig {
            packs:scratch.path().join("worker-cache"),runtimes:runtime_root.clone(),environment:env.clone(),filter:None,
        },worker_stop.clone()));
        let completed=wait_for_status(pool,candidate.id,"activating").await?;
        assert_eq!(completed.assigned_worker_id,Some(old_worker));
        let summary=completed.test_result.context("Candidate test did not run")?;
        assert_eq!(summary["passed"],1);
        assert_eq!(summary["testSuites"][0]["testCases"][0]["stdout"].as_str().unwrap().len(),1048576);
        assert_eq!(summary["testSuites"][0]["testCases"][0]["stderr"].as_str().unwrap().len(),1048576);
        assert_eq!(list().await?.status(),StatusCode::FORBIDDEN);
        wait_for_status(pool,preparation.id,"activating").await?;
        assert_eq!(PackInstallRepository::new(pool.clone()).find_by_id(preparation.id).await?.unwrap().assigned_worker_id,Some(old_worker));
        let rows: Vec<(i32, String)> = sqlx::query_as("SELECT attempt,finish_digest FROM manager_pack_job WHERE install=ANY($1) ORDER BY install")
            .bind([candidate.id,preparation.id]).fetch_all(pool).await?;
        assert_eq!(rows.len(),2);
        assert!(rows.iter().all(|row| row.0==2));
        assert!(rows[1].1.starts_with("archive:"));

        let mut tx=pool.begin().await?;
        Preparations::bind_release(&mut tx,&[preparation.id],activation.id,&release,std::slice::from_ref(&job)).await?;
        PackInstallRepository::finish_activation_in_transaction(&mut tx,activation.id,release.pack).await?.context("Preparation remained unpublished")?;
        tx.commit().await?;
        let ready=Preparations::ready_for(pool,release.id,&release.digest,&job.target).await?.pop().context("Prepared archive not bound")?;
        let current=api.enroll(ACTION_SECRET,&replacement_slot).await?;
        let committed: Uuid=sqlx::query_scalar("SELECT claim FROM manager_pack_job WHERE install=$1")
            .bind(preparation.id).fetch_one(pool).await?;
        let archive = ready.archive().context("Prepared archive was not published")?;
        let bytes = futures::TryStreamExt::try_fold(
            paused.inner.get_pinned(&ObjectKey::new(&archive.key)?,&ProviderVersion::from_stored(&archive.version)?,
                archive.size,hex::decode(&archive.sha256)?.try_into().unwrap(),None).await?,
            Vec::new(), |mut bytes,chunk| async move {bytes.extend_from_slice(&chunk);Ok(bytes)},
        ).await?;
        let mut metadata=serde_json::to_value(&ready)?;
        metadata.as_object_mut().unwrap().remove("archive");
        let receipt: attune_common::runtime_lifecycle::InstalledPreparation=serde_json::from_value(metadata)?;
        let replay=|receipt:&attune_common::runtime_lifecycle::InstalledPreparation| -> Result<_> {
            let receipt=serde_json::to_vec(receipt)?;
            let mut body=(receipt.len() as u32).to_be_bytes().to_vec();
            body.extend(receipt);
            body.extend_from_slice(&bytes);
            Ok(http.post(format!("{url}/api/v1/internal/pack-installs/{}/prepared-runtime",preparation.id))
                .bearer_auth(&current.token)
                .header(attune_common::manager_control::PACK_JOB_HEADER,preparation.id.to_string())
                .header(PACK_JOB_CLAIM_HEADER,committed.to_string())
                .header("x-attune-runtime-archive-size",archive.size)
                .header("x-attune-sha256",&archive.sha256)
                .body(body).send())
        };
        let (one,two)=tokio::try_join!(replay(&receipt)?,replay(&receipt)?)?;
        assert_eq!(one.status(),StatusCode::NO_CONTENT);
        assert_eq!(two.status(),StatusCode::NO_CONTENT);
        let mut divergent=receipt;
        divergent.target.reference.push_str("-other");
        assert_eq!(replay(&divergent)?.await?.status(),StatusCode::FORBIDDEN);
        assert_eq!(Preparations::ready_for(pool,release.id,&release.digest,&job.target).await?[0].archive(),ready.archive());
        worker_stop.cancel();
        task.await??;
        let sensor_session=Arc::new(api.enroll(SENSOR_SECRET,&enrollment(WorkerRole::Sensor,"prepared-consumer")).await?);
        let sensor_worker=report(&api,&sensor_session,&env,&runtime_root,false).await?;
        let AcquireSensorWorkloadOutcome::Acquired(lease)=SensorWorkloadRepository::acquire_or_renew(pool,AcquireSensorWorkloadInput {
            sensor_id:sensor.id,worker_id:sensor_worker,worker_instance:sensor_session.instance,lease_seconds:60,
        }).await? else {anyhow::bail!("No sensor assignment");};
        let workload=SensorWorkloadRepository::begin_process(pool,lease).await?.context("Sensor process did not start")?;
        let mut tx=pool.begin().await?;
        SchemaTypeRepository::lock_selection(&mut tx).await?;
        SensorSchemaRepository::admit(&mut tx,&workload,encryption.as_deref()).await?;
        tx.commit().await?;
        let (_,session)=watch::channel(sensor_session.clone());
        let sensor_cache=scratch.path().join("sensor-cache");
        let transfer=api.pack_transport(session,PackTransferScope::Sensor {workload:workload.workload_id,generation:workload.generation},sensor_cache.to_str().unwrap());
        transfer.sync_release(&release.pack_ref,release.id,&release.digest).await?;
        let prepared_url=format!("{url}/api/v1/internal/pack-releases/{}/prepared-runtimes/{}",release.id,ready.directory());
        let get=|generation:i64|http.get(&prepared_url).bearer_auth(&sensor_session.token)
            .header(attune_common::manager_control::WORK_SCOPE_HEADER,workload.workload_id.to_string())
            .header(attune_common::manager_control::SENSOR_GENERATION_HEADER,generation.to_string()).send();
        assert_eq!(get(workload.generation+1).await?.status(),StatusCode::FORBIDDEN);
        assert!(ready.archive().unwrap().version.starts_with("v:"));
        // A new volume mounted at the same configured absolute path is empty.
        std::fs::remove_dir_all(&runtime_root)?;
        assert_eq!(PreparationScope::open(&runtime_root,&prep_env)?.as_ref(),Some(&ready.scope));
        transfer.sync_prepared_runtime(release.id,&ready).await?;
        let invocation=job.recipe.open_installed(&ready,&runtime_root,&prep_env).await?;
        let mut command=Command::new(&invocation.command[0]);
        prep_env.apply(&mut command);
        command.args(&invocation.command[1..]).envs(&invocation.env).current_dir(&invocation.working_dir);
        let output=invocation.sandbox_invocation(command,&prep_env)?.output()?;
        invocation.cleanup().await?;
        assert!(output.status.success());
        assert_eq!(output.stdout,b"prepared");
        api.withdraw(&sensor_session).await?;
        assert!(matches!(get(workload.generation).await?.status(),StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN));
        Ok(())
    }.await;
    stop.cancel();
    workers.close();
    workers.wait().await;
    processor.await??;
    server.await??;
    broker.close().await?;
    db.cleanup().await?;
    result
}
