//! Repository-only candidate evidence regressions. No provider is contacted;
//! these do not establish native launch or packaged acceptance.
mod helpers;
use anyhow::{ensure, Result};
use attune_common::{
    auth::hash_integration_token,
    blob_store::{ObjectKey, ProviderVersion, StoredObject},
    isolated_process::{ProcessCreateDisposition, ProcessSpec, ResourceIdentity},
    models::enums::{WorkerStatus, WorkerType},
    repositories::{
        candidate_effect::{CandidateArchiveBinding, CandidateEffectRepository},
        candidate_process::{
            CandidateCapture, CandidateEffectsSource, CandidateProcessRepository as Ledger,
            CandidateSource,
        },
        execution_process::ExecutionProcessRepository as Effects,
        object_maintenance::ObjectMaintenanceRepository,
        runtime::{CreateWorkerInput, WorkerRepository},
        Create, PackInstallRepository,
    },
    workload_launch::*,
};
use uuid::Uuid;
const MASTER: &str = "candidate-process-repository-test-master-key";
const TOKEN: &str = "owned-candidate-archive-token";
async fn fixture() -> Result<(
    attune_common::test_database::TestDatabase,
    CandidateSource,
    ProcessSpec,
)> {
    let db = helpers::create_test_pool().await?;
    let instance = Uuid::new_v4();
    let worker = WorkerRepository::create(
        &*db,
        CreateWorkerInput {
            name: format!("candidate-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(serde_json::json!({})),
            meta: Some(serde_json::json!({"registration_incarnation":instance.to_string()})),
        },
    )
    .await?;
    WorkerRepository::heartbeat_incarnation(db.pool(), worker.id, instance).await?;
    let pack = helpers::PackFixture::new_unique("native_candidate")
        .create(&db)
        .await?;
    let install = PackInstallRepository::new(db.pool().clone())
        .create(&pack.r#ref, "1.0.0", "install", Some(pack.id), None)
        .await?;
    let object = StoredObject {
        key: ObjectKey::new(format!("packs/candidates/{}.tar.gz", install.id))?,
        provider_version: ProviderVersion::from_stored("v:owned")?,
        size: 123,
        sha256: [0xab; 32],
    };
    let mut tx = db.begin().await?;
    ObjectMaintenanceRepository::reserve_upload(&mut *tx, object.key.as_str(), "pack").await?;
    ObjectMaintenanceRepository::record_uploaded(
        &mut *tx,
        object.key.as_str(),
        object.provider_version.as_stored(),
        object.size as i64,
    )
    .await?;
    ensure!(PackInstallRepository::attach_candidate_object(&mut tx, install.id, &object).await?);
    tx.commit().await?;
    PackInstallRepository::new(db.pool().clone())
        .claim_worker(install.id, worker.id, Some(&hash_integration_token(TOKEN)))
        .await?
        .unwrap();
    let source = Ledger::admit(db.pool(), install.id, worker.id, instance, TOKEN).await?;
    let pin = |kind, id| PinnedLaunchInput {
        pin_id: id,
        kind,
        object_id: id,
        object_version: "owned-pin".into(),
        size_bytes: 1,
        sha256: Sha256Digest::of_bytes(b"owned"),
        max_expanded_bytes: 1024,
        max_files: 2,
    };
    let image = PinnedImage {
        reference: format!("owned.invalid/candidate@sha256:{}", "a".repeat(64)),
        sha256: Sha256Digest::parse("a".repeat(64))?,
    };
    let spec = ProcessSpec {
        intent: Uuid::new_v4(),
        purpose: LaunchPurpose::CandidateTest,
        target: LaunchTarget::CandidateTest {
            install: install.id,
            test_job: 1,
            attempt: instance,
        },
        image,
        platform: LaunchPlatform::LinuxAmd64,
        inputs: vec![
            pin(LaunchInputKind::Pack, install.id),
            pin(LaunchInputKind::Runner, install.id + 1),
        ],
        pack_access: PackAccess::CandidateCopy,
        stage: None,
        timeout_ms: Some(1000),
        invocation: LaunchInvocation {
            argv: vec!["/bin/sh".into(), format!("{PACK_ROOT}/test.sh")],
            env: Default::default(),
            working_root: LaunchRoot::Pack,
        },
    };
    Ok((db, source, spec))
}
fn resource() -> ResourceIdentity {
    ResourceIdentity::Docker {
        engine_id: 1,
        daemon_id: "owned-daemon".into(),
        container_id: "a".repeat(64),
    }
}
#[tokio::test]
async fn candidate_create_and_stdin_are_one_shot_after_unknown_ack() -> Result<()> {
    let (db, source, spec) = fixture().await?;
    assert!(Ledger::retained(db.pool(), &source, &spec.target, MASTER)
        .await?
        .is_none());
    let spec = Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    let retained = Ledger::retained(db.pool(), &source, &spec.target, MASTER)
        .await?
        .unwrap();
    assert_eq!(retained.intent, spec.intent);
    assert_eq!(retained.purpose, spec.purpose);
    assert_eq!(retained.target, spec.target);
    assert_eq!(retained.image, spec.image);
    assert_eq!(retained.platform, spec.platform);
    assert_eq!(retained.inputs, spec.inputs);
    assert_eq!(retained.pack_access, spec.pack_access);
    assert!(retained.stage.is_none());
    assert_eq!(retained.timeout_ms, spec.timeout_ms);
    assert_eq!(retained.invocation, spec.invocation);
    assert_eq!(
        Ledger::begin_create(db.pool(), spec.intent, MASTER).await?,
        ProcessCreateDisposition::CreateOnce
    );
    assert_eq!(
        Ledger::begin_create(db.pool(), spec.intent, MASTER).await?,
        ProcessCreateDisposition::ReconcileOnly
    );
    let remaining = Ledger::remaining(db.pool(), spec.intent, MASTER).await?;
    assert!(remaining <= std::time::Duration::from_secs(1));
    let resource = resource();
    Effects::record_resource(db.pool(), spec.intent, &resource).await?;
    assert!(
        Ledger::consume_stdin(db.pool(), spec.intent, &resource, MASTER)
            .await?
            .as_bytes()
            .is_empty()
    );
    assert!(
        Ledger::consume_stdin(db.pool(), spec.intent, &resource, MASTER)
            .await
            .is_err()
    );
    Ledger::settle_unsubmitted(db.pool(), &source).await?;
    assert!(!Ledger::settled(db.pool(), &source, MASTER).await?);
    let mut next = spec.clone();
    next.intent = Uuid::new_v4();
    if let LaunchTarget::CandidateTest { test_job, .. } = &mut next.target {
        *test_job = 2;
    }
    assert!(Ledger::reserve(db.pool(), &source, &next, MASTER)
        .await
        .is_err());
    db.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn worker_restore_cannot_readmit_or_replace_an_unsettled_candidate() -> Result<()> {
    let (db, source, spec) = fixture().await?;
    let spec = Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    Ledger::begin_create(db.pool(), spec.intent, MASTER).await?;
    sqlx::query("UPDATE worker SET status='inactive' WHERE id=$1")
        .bind(source.worker)
        .execute(&*db)
        .await?;
    sqlx::query("UPDATE worker SET status='active' WHERE id=$1")
        .bind(source.worker)
        .execute(&*db)
        .await?;
    assert!(Ledger::validate(db.pool(), spec.intent, MASTER)
        .await
        .is_err());
    assert!(Ledger::retained(db.pool(), &source, &spec.target, MASTER)
        .await
        .is_err());
    assert!(Ledger::admit(
        db.pool(),
        source.install,
        source.worker,
        source.instance,
        TOKEN
    )
    .await
    .is_err());
    db.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn opt_in_candidate_lease_revocation_stops_native_effect_authority() -> Result<()> {
    let (db, mut source, spec) = fixture().await?;
    let lease = CandidateEffectRepository::open(
        db.pool(),
        source.install,
        source.worker,
        TOKEN,
        &CandidateArchiveBinding {
            key: &source.archive_key,
            provider_version: &source.archive_version,
            digest: &source.archive_digest,
        },
    )
    .await?;
    source.effects = Some(CandidateEffectsSource {
        lease: lease.id,
        token_hash: hash_integration_token(&lease.token),
    });
    let spec = Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    CandidateEffectRepository::close(db.pool(), lease.id).await?;
    assert!(Ledger::begin_create(db.pool(), spec.intent, MASTER)
        .await
        .is_err());
    db.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn captured_results_require_stdin_exit_and_exact_removal_and_are_encrypted() -> Result<()> {
    let (db, source, spec) = fixture().await?;
    let spec = Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    let resource = resource();
    Ledger::begin_create(db.pool(), spec.intent, MASTER).await?;
    Effects::record_resource(db.pool(), spec.intent, &resource).await?;
    Ledger::consume_stdin(db.pool(), spec.intent, &resource, MASTER).await?;
    let capture = CandidateCapture {
        interruption: None,
        exit_code: 0,
        stdout: "private-candidate-output".into(),
        stderr: "private-error-stream".into(),
        duration_ms: 1,
    };
    assert!(
        Ledger::capture(db.pool(), spec.intent, &resource, &capture, MASTER)
            .await
            .is_err()
    );
    Effects::record_exit(
        db.pool(),
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await?;
    Effects::request_stop(db.pool(), spec.intent).await?;
    assert!(
        Ledger::capture(db.pool(), spec.intent, &resource, &capture, MASTER)
            .await
            .is_err()
    );
    Ledger::acknowledge_removed(db.pool(), spec.intent, &resource).await?;
    Ledger::capture(db.pool(), spec.intent, &resource, &capture, MASTER).await?;
    let encrypted: String = sqlx::query_scalar(
        "SELECT candidate_capture FROM execution_process_effect WHERE intent=$1",
    )
    .bind(spec.intent)
    .fetch_one(&*db)
    .await?;
    assert!(!encrypted.contains(&capture.stdout));
    assert_eq!(
        Ledger::captured(db.pool(), spec.intent, MASTER)
            .await?
            .unwrap()
            .stdout,
        capture.stdout
    );
    assert!(Ledger::settled(db.pool(), &source, MASTER).await?);
    sqlx::query("UPDATE worker SET status='inactive' WHERE id=$1")
        .bind(source.worker)
        .execute(&*db)
        .await?;
    sqlx::query("UPDATE worker SET status='active' WHERE id=$1")
        .bind(source.worker)
        .execute(&*db)
        .await?;
    assert!(Ledger::admit(
        db.pool(),
        source.install,
        source.worker,
        source.instance,
        TOKEN
    )
    .await
    .is_err());
    db.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn never_submitted_cleanup_is_not_unknown_create_cleanup() -> Result<()> {
    let (db, source, spec) = fixture().await?;
    let spec = Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    Ledger::settle_unsubmitted(db.pool(), &source).await?;
    assert!(Ledger::settled(db.pool(), &source, MASTER).await?);
    assert!(Ledger::begin_create(db.pool(), spec.intent, MASTER)
        .await
        .is_err());
    assert!(Ledger::captured(db.pool(), spec.intent, MASTER)
        .await?
        .is_none());
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn wrong_active_worker_cannot_admit_candidate_source() -> Result<()> {
    let (db, source, _) = fixture().await?;
    let instance = Uuid::new_v4();
    let other = WorkerRepository::create(
        &*db,
        CreateWorkerInput {
            name: format!("wrong-candidate-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(serde_json::json!({})),
            meta: Some(serde_json::json!({"registration_incarnation":instance.to_string()})),
        },
    )
    .await?;
    WorkerRepository::heartbeat_incarnation(db.pool(), other.id, instance).await?;
    assert!(
        Ledger::admit(db.pool(), source.install, other.id, instance, TOKEN)
            .await
            .is_err()
    );
    db.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn result_admission_checks_database_time_after_observed_install_lock_wait() -> Result<()> {
    use std::time::Duration;
    let (db, source, _) = fixture().await?;
    let pool = db.pool().clone();
    let mut blocker = pool.begin().await?;
    sqlx::query("SELECT id FROM pack_install WHERE id=$1 FOR UPDATE")
        .bind(source.install)
        .execute(&mut *blocker)
        .await?;
    // Fixture construction/migration is complete before starting this actual clock.
    sqlx::query(
        "UPDATE worker SET last_heartbeat=clock_timestamp()-interval '85 seconds' WHERE id=$1",
    )
    .bind(source.worker)
    .execute(&pool)
    .await?;
    let worker = source.worker;
    let test_pool = pool.clone();
    let (signal, started) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut tx = test_pool.begin().await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await?;
        let _ = signal.send(pid);
        let result = Ledger::lock_result(&mut tx, &source, MASTER).await;
        tx.rollback().await?;
        Ok::<_, anyhow::Error>(result.is_err())
    });
    let observed=async {
  let pid=tokio::time::timeout(Duration::from_secs(10),started).await??;
  tokio::time::timeout(Duration::from_secs(10),async {
   loop {let blocked:bool=sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1))>0").bind(pid).fetch_one(&pool).await?;if blocked{break Ok::<_,anyhow::Error>(())}tokio::time::sleep(Duration::from_millis(10)).await;}
  }).await??;
  tokio::time::timeout(Duration::from_secs(10),async {
   loop {let expired:bool=sqlx::query_scalar("SELECT clock_timestamp()>=last_heartbeat+interval '90 seconds' FROM worker WHERE id=$1").bind(worker).fetch_one(&pool).await?;if expired{break Ok::<_,anyhow::Error>(())}tokio::time::sleep(Duration::from_millis(10)).await;}
  }).await??;Ok::<_,anyhow::Error>(())
 }.await;
    blocker.rollback().await?;
    let denied = task.await?;
    let cleanup = db.cleanup().await;
    observed?;
    assert!(denied?);
    cleanup?;
    Ok(())
}
async fn completed_with_lease() -> Result<(
    attune_common::test_database::TestDatabase,
    CandidateSource,
    Uuid,
    String,
)> {
    let (db, mut source, spec) = fixture().await?;
    let lease = CandidateEffectRepository::open(
        db.pool(),
        source.install,
        source.worker,
        TOKEN,
        &CandidateArchiveBinding {
            key: &source.archive_key,
            provider_version: &source.archive_version,
            digest: &source.archive_digest,
        },
    )
    .await?;
    source.effects = Some(CandidateEffectsSource {
        lease: lease.id,
        token_hash: hash_integration_token(&lease.token),
    });
    let spec = Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    let resource = resource();
    Ledger::begin_create(db.pool(), spec.intent, MASTER).await?;
    Effects::record_resource(db.pool(), spec.intent, &resource).await?;
    Ledger::consume_stdin(db.pool(), spec.intent, &resource, MASTER).await?;
    Effects::record_exit(
        db.pool(),
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 1,
            oom_killed: false,
        },
    )
    .await?;
    Effects::request_stop(db.pool(), spec.intent).await?;
    Ledger::acknowledge_removed(db.pool(), spec.intent, &resource).await?;
    Ledger::capture(
        db.pool(),
        spec.intent,
        &resource,
        &CandidateCapture {
            interruption: None,
            exit_code: 1,
            stdout: "Total Tests: 1\nPassed: 0\nFailed: 1\n".into(),
            stderr: "known failed suite".into(),
            duration_ms: 1,
        },
        MASTER,
    )
    .await?;
    Ok((db, source, lease.id, lease.token))
}
fn result(source: &CandidateSource) -> attune_common::models::PackTestResult {
    attune_common::models::PackTestResult {
        pack_ref: source.pack_ref.clone(),
        pack_version: source.pack_version.clone(),
        execution_time: chrono::Utc::now(),
        status: "failed".into(),
        total_tests: 1,
        passed: 0,
        failed: 1,
        skipped: 0,
        pass_rate: 0.0,
        duration_ms: 1,
        test_suites: vec![],
    }
}
#[tokio::test]
async fn source_fenced_result_close_and_finish_rollback_together() -> Result<()> {
    use attune_common::repositories::PackTestRepository;
    let (db, source, lease, token) = completed_with_lease().await?;
    let result = result(&source);
    let mut tx = db.begin().await?;
    let fence = Ledger::lock_result(&mut tx, &source, MASTER).await?;
    CandidateEffectRepository::close_on(&mut tx, lease).await?;
    let record = PackTestRepository::create_on(
        &mut tx,
        source.pack_id.unwrap(),
        &source.pack_version,
        "install",
        &result,
    )
    .await?;
    assert!(PackInstallRepository::finish_running_on(
        &mut tx,
        source.install,
        attune_common::models::PackInstallStatus::Activating,
        Some(record.id),
        Some(serde_json::to_value(&result)?),
        None
    )
    .await?
    .is_some());
    fence.ensure_fresh(&mut tx).await?;
    tx.rollback().await?;
    assert!(
        CandidateEffectRepository::authenticate(db.pool(), lease, &token)
            .await
            .is_ok()
    );
    let stored = PackInstallRepository::new(db.pool().clone())
        .find_by_id(source.install)
        .await?
        .unwrap();
    assert_eq!(stored.status, "running");
    assert!(stored.test_result.is_none());
    assert!(PackTestRepository::new(db.pool().clone())
        .list_by_pack(source.pack_id.unwrap(), 10, 0)
        .await?
        .is_empty());
    db.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn committed_result_survives_lost_caller_ack_without_reexecuting_or_refinishing() -> Result<()>
{
    use attune_common::repositories::PackTestRepository;
    let (db, source, lease, token) = completed_with_lease().await?;
    let result = result(&source);
    let mut tx = db.begin().await?;
    let fence = Ledger::lock_result(&mut tx, &source, MASTER).await?;
    CandidateEffectRepository::close_on(&mut tx, lease).await?;
    let record = PackTestRepository::create_on(
        &mut tx,
        source.pack_id.unwrap(),
        &source.pack_version,
        "install",
        &result,
    )
    .await?;
    PackInstallRepository::finish_running_on(
        &mut tx,
        source.install,
        attune_common::models::PackInstallStatus::Activating,
        Some(record.id),
        Some(serde_json::to_value(&result)?),
        None,
    )
    .await?
    .unwrap();
    fence.ensure_fresh(&mut tx).await?;
    tx.commit().await?;
    // Discard the caller's success value, then reconcile only committed domain
    // state. Never repeat process effects or publish a second test record.
    let stored = PackInstallRepository::new(db.pool().clone())
        .find_by_id(source.install)
        .await?
        .unwrap();
    assert_eq!(stored.test_execution_id, Some(record.id));
    assert_eq!(stored.status, "activating");
    assert!(
        CandidateEffectRepository::authenticate(db.pool(), lease, &token)
            .await
            .is_err()
    );
    let mut retry = db.begin().await?;
    assert!(Ledger::lock_result(&mut retry, &source, MASTER)
        .await
        .is_err());
    retry.rollback().await?;
    assert_eq!(
        PackTestRepository::new(db.pool().clone())
            .list_by_pack(source.pack_id.unwrap(), 10, 0)
            .await?
            .len(),
        1
    );
    db.cleanup().await?;
    Ok(())
}

// Each fixture owns a separate database; `core.python` is the actual fixed
// business key being selected, not a process-global test configuration.
async fn python_fixture() -> Result<(
    attune_common::test_database::TestDatabase,
    CandidateSource,
    ProcessSpec,
)> {
    use attune_common::repositories::{
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::{CreateRuntimeInput, RuntimeRepository},
        runtime_version::{CreateRuntimeVersionInput, RuntimeVersionRepository},
    };
    let (db, source, spec) = fixture().await?;
    let pack = helpers::PackFixture::new("core").create(&db).await?;
    let metadata: serde_json::Value =
        serde_yaml_ng::from_str(include_str!("../../../packs/core/runtimes/python.yaml"))?;
    let config = metadata["execution_config"].clone();
    let runtime = RuntimeRepository::create(
        db.pool(),
        CreateRuntimeInput {
            r#ref: "core.python".into(),
            pack: Some(pack.id),
            pack_ref: Some("core".into()),
            description: None,
            name: "Python".into(),
            aliases: vec![],
            distributions: metadata["distributions"].clone(),
            installation: None,
            execution_config: config.clone(),
            auto_detected: false,
            detection_config: serde_json::json!({}),
        },
    )
    .await?;
    // A newer registered version must not replace the base configuration used
    // by prepare_python_candidate_test_runtime.
    RuntimeVersionRepository::create(db.pool(), CreateRuntimeVersionInput {
        runtime: runtime.id, runtime_ref: "core.python".into(), version: "99.0.0".into(),
        version_major: Some(99), version_minor: Some(0), version_patch: Some(0),
        execution_config: serde_json::json!({"interpreter":{"binary":"forbidden-highest-version"}}),
        distributions: serde_json::json!({}), is_default: true, available: true, meta: serde_json::json!({}),
    }).await?;
    let mut tx = db.begin().await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: "core".into(),
            version: "1.0.0".into(),
            digest: "d".repeat(64),
            object_key: "owned-provider-source".into(),
            provider_version: "owned-version".into(),
            content_path: "/owned/not-read-by-repository-test".into(),
            archive_size: 1,
            manifest: serde_json::json!({}),
        },
    )
    .await?;
    PackReleaseRepository::activate(&mut tx, pack.id, release.id).await?;
    tx.commit().await?;
    let source = Ledger::admit_python(db.pool(), source).await?;
    assert_eq!(source.python.as_ref().unwrap().runtime_config, config);
    assert_eq!(source.python.as_ref().unwrap().release, release.id);
    Ok((db, source, spec))
}

#[tokio::test]
async fn python_base_admission_recovers_exact_epochs_and_cannot_be_downgraded() -> Result<()> {
    let (db, source, spec) = python_fixture().await?;
    let spec = Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    let base = Ledger::admit(
        db.pool(),
        source.install,
        source.worker,
        source.instance,
        TOKEN,
    )
    .await?;
    let recovered = Ledger::admit_python(db.pool(), base.clone()).await?;
    let retained = Ledger::retained(db.pool(), &recovered, &spec.target, MASTER)
        .await?
        .unwrap();
    assert_eq!(retained.intent, spec.intent);
    Ledger::settle_unsubmitted(db.pool(), &source).await?;
    let mut next = spec.clone();
    next.intent = Uuid::new_v4();
    if let LaunchTarget::CandidateTest { test_job, .. } = &mut next.target {
        *test_job = 2;
    }
    assert!(Ledger::reserve(db.pool(), &base, &next, MASTER)
        .await
        .is_err());
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn python_definition_restore_never_restores_candidate_effect_authority() -> Result<()> {
    let (db, source, spec) = python_fixture().await?;
    Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    let proof = source.python.as_ref().unwrap();
    sqlx::query("UPDATE runtime SET execution_config=$2 WHERE id=$1")
        .bind(proof.runtime)
        .bind(serde_json::json!({"interpreter":{"binary":"changed"}}))
        .execute(db.pool())
        .await?;
    sqlx::query("UPDATE runtime SET execution_config=$2 WHERE id=$1")
        .bind(proof.runtime)
        .bind(&proof.runtime_config)
        .execute(db.pool())
        .await?;
    assert!(Ledger::validate_source(db.pool(), &source).await.is_err());
    let base = Ledger::admit(
        db.pool(),
        source.install,
        source.worker,
        source.instance,
        TOKEN,
    )
    .await?;
    assert!(Ledger::admit_python(db.pool(), base).await.is_err());
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn python_provider_release_restore_never_restores_candidate_effect_authority() -> Result<()> {
    let (db, source, spec) = python_fixture().await?;
    Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    let proof = source.python.as_ref().unwrap();
    sqlx::query("UPDATE pack SET active_release=NULL WHERE id=$1")
        .bind(proof.provider_pack)
        .execute(db.pool())
        .await?;
    sqlx::query("UPDATE pack SET active_release=$2 WHERE id=$1")
        .bind(proof.provider_pack)
        .bind(proof.release)
        .execute(db.pool())
        .await?;
    assert!(Ledger::validate_source(db.pool(), &source).await.is_err());
    assert!(Ledger::retained(db.pool(), &source, &spec.target, MASTER)
        .await
        .is_err());
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn unverified_preparation_capture_settles_cleanup_but_never_fabricates_test_history(
) -> Result<()> {
    let (db, source, spec) = fixture().await?;
    let spec = Ledger::reserve(db.pool(), &source, &spec, MASTER).await?;
    let resource = resource();
    Ledger::begin_create(db.pool(), spec.intent, MASTER).await?;
    Effects::record_resource(db.pool(), spec.intent, &resource).await?;
    Ledger::consume_stdin(db.pool(), spec.intent, &resource, MASTER).await?;
    Effects::record_exit(
        db.pool(),
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 1,
            oom_killed: false,
        },
    )
    .await?;
    Effects::request_stop(db.pool(), spec.intent).await?;
    Ledger::acknowledge_removed(db.pool(), spec.intent, &resource).await?;
    Ledger::capture(
        db.pool(),
        spec.intent,
        &resource,
        &CandidateCapture {
            interruption: Some(
                attune_common::test_executor::TestProcessInterruption::PreparationUnverified,
            ),
            exit_code: 1,
            stdout: String::new(),
            stderr: "static helper failure".into(),
            duration_ms: 1,
        },
        MASTER,
    )
    .await?;
    let captured = Ledger::captured(db.pool(), spec.intent, MASTER)
        .await?
        .unwrap();
    assert_eq!(
        captured.interruption,
        Some(attune_common::test_executor::TestProcessInterruption::PreparationUnverified)
    );
    assert!(Ledger::settled(db.pool(), &source, MASTER).await?);
    let mut tx = db.begin().await?;
    let fence = Ledger::lock_result(&mut tx, &source, MASTER).await?;
    fence.ensure_fresh(&mut tx).await?;
    let history: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pack_test_execution WHERE pack_id=$1")
            .bind(source.pack_id.unwrap())
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(history, 0);
    tx.rollback().await?;
    db.cleanup().await?;
    Ok(())
}
