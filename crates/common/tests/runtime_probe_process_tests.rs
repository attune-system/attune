//! Runtime-only native discovery ledger; no provider is contacted by this suite.
mod helpers;
use attune_common::{
    isolated_process::{ProcessCreateDisposition, ProcessSpec, ResourceIdentity},
    models::enums::{WorkerStatus, WorkerType},
    repositories::{
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::{CreateRuntimeInput, CreateWorkerInput, RuntimeRepository, WorkerRepository},
        runtime_probe_process::{
            RuntimeProbeProcessRepository as Probe, RuntimeProbeSource, VerifiedToolMap,
        },
        Create,
    },
    runtime_lifecycle::{RuntimeRecipe, VerifiedTool},
    workload_launch::*,
};
use helpers::{create_test_pool, PackFixture};
use serde_json::json;
use uuid::Uuid;
const MASTER: &str = "owned-runtime-probe-test-master-key-32-bytes";
async fn fixture() -> (
    attune_common::test_database::TestDatabase,
    RuntimeProbeSource,
    ProcessSpec,
) {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("native_probe")
        .create(&db)
        .await
        .unwrap();
    let recipe:RuntimeRecipe=serde_json::from_value(json!({"tools":[{"name":"owned","binaries":["/usr/bin/owned"],"version_pattern":"(?<version>1)","primary":true}],"launch":{"command":["{tool_owned}"]}})).unwrap();
    let config = json!({"lifecycle":recipe});
    let runtime = RuntimeRepository::create(
        &*db,
        CreateRuntimeInput {
            r#ref: format!("{}.owned", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            description: None,
            name: "owned".into(),
            aliases: vec![],
            distributions: json!({}),
            installation: None,
            execution_config: config.clone(),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await
    .unwrap();
    let instance = Uuid::new_v4();
    let worker = WorkerRepository::create(
        &*db,
        CreateWorkerInput {
            name: format!("probe-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: Some(json!({"registration_incarnation":instance.to_string()})),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE worker SET last_heartbeat=clock_timestamp() WHERE id=$1")
        .bind(worker.id)
        .execute(&*db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref,
            version: "1.0.0".into(),
            digest: "a".repeat(64),
            object_key: "owned".into(),
            provider_version: "owned".into(),
            content_path: "/owned/retained".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate(&mut tx, pack.id, release.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let input = |kind, pin_id| PinnedLaunchInput {
        kind,
        pin_id,
        object_id: release.id,
        object_version: release.digest.clone(),
        size_bytes: 1,
        sha256: Sha256Digest::parse("b".repeat(64)).unwrap(),
        max_expanded_bytes: 1,
        max_files: 1,
    };
    let spec = ProcessSpec {
        intent: Uuid::new_v4(),
        purpose: LaunchPurpose::RuntimeProbe,
        target: LaunchTarget::RuntimeProbe {
            admission: runtime.id,
            attempt: Uuid::new_v4(),
        },
        image: PinnedImage {
            reference: format!("fixture.invalid/image@sha256:{}", "c".repeat(64)),
            sha256: Sha256Digest::parse("c".repeat(64)).unwrap(),
        },
        platform: LaunchPlatform::LinuxAmd64,
        inputs: vec![
            input(LaunchInputKind::Pack, 1),
            input(LaunchInputKind::Runner, 2),
        ],
        pack_access: PackAccess::ReadOnly,
        stage: None,
        timeout_ms: Some(1000),
        invocation: LaunchInvocation {
            argv: vec![
                format!("{RUNNER_ROOT}/linux-amd64"),
                "inspect-runtime-tools".into(),
                "/usr/bin:/bin".into(),
                serde_json::to_string(&recipe).unwrap(),
            ],
            env: Default::default(),
            working_root: LaunchRoot::Pack,
        },
    };
    let source = RuntimeProbeSource {
        worker: worker.id,
        incarnation: instance,
        runtime: runtime.id,
        runtime_version: None,
        runtime_version_string: None,
        provider_pack: pack.id,
        release: release.id,
        release_digest: release.digest,
        runtime_config: config,
        version_config: None,
        runtime_distributions: json!({}),
        version_distributions: None,
    };
    (db, source, spec)
}
fn resource() -> ResourceIdentity {
    ResourceIdentity::Kubernetes {
        cluster: "owned".into(),
        namespace: "owned".into(),
        pod_name: "owned".into(),
        pod_uid: Uuid::new_v4().to_string(),
    }
}
fn tools() -> VerifiedToolMap {
    std::collections::BTreeMap::from([(
        "owned".into(),
        VerifiedTool {
            executable: "/usr/bin/owned".into(),
            identity: "/usr/bin/owned".into(),
            version: "1".into(),
        },
    )])
}
#[tokio::test]
async fn replay_unknown_create_and_empty_stdin_are_one_shot() {
    let (db, source, spec) = fixture().await;
    let attempt = Uuid::new_v4();
    let spec = Probe::reserve(&db, attempt, source.clone(), spec, MASTER)
        .await
        .unwrap();
    let mut retry = spec.clone();
    retry.intent = Uuid::new_v4();
    assert_eq!(
        Probe::reserve(&db, attempt, source, retry, MASTER)
            .await
            .unwrap()
            .intent,
        spec.intent
    );
    assert_eq!(
        Probe::begin_create(&db, spec.intent, MASTER).await.unwrap(),
        ProcessCreateDisposition::CreateOnce
    );
    assert_eq!(
        Probe::begin_create(&db, spec.intent, MASTER).await.unwrap(),
        ProcessCreateDisposition::ReconcileOnly
    );
    Probe::request_stop(&db, spec.intent).await.unwrap();
    assert!(Probe::acknowledge_removed(&db, spec.intent, None)
        .await
        .is_err());
    assert!(Probe::tools(&db, spec.intent, MASTER).await.is_err());
    db.cleanup().await.unwrap();
}
#[tokio::test]
async fn runtime_revoke_restore_and_worker_expiry_deny_old_probe() {
    let (db, source, spec) = fixture().await;
    let spec = Probe::reserve(&db, Uuid::new_v4(), source.clone(), spec, MASTER)
        .await
        .unwrap();
    sqlx::query("UPDATE runtime SET execution_config='{}' WHERE id=$1")
        .bind(source.runtime)
        .execute(&*db)
        .await
        .unwrap();
    sqlx::query("UPDATE runtime SET execution_config=$2 WHERE id=$1")
        .bind(source.runtime)
        .bind(&source.runtime_config)
        .execute(&*db)
        .await
        .unwrap();
    assert!(Probe::validate_source(&db, spec.intent, MASTER)
        .await
        .is_err());
    let mut next = spec.clone();
    next.intent = Uuid::new_v4();
    let next = Probe::reserve(&db, Uuid::new_v4(), source.clone(), next, MASTER)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE worker SET last_heartbeat=clock_timestamp()-interval '91 seconds' WHERE id=$1",
    )
    .bind(source.worker)
    .execute(&*db)
    .await
    .unwrap();
    assert!(Probe::begin_create(&db, next.intent, MASTER).await.is_err());
    db.cleanup().await.unwrap();
}
#[tokio::test]
async fn negative_exit_and_missing_capture_never_advertise_tools() {
    let (db, source, spec) = fixture().await;
    let spec = Probe::reserve(&db, Uuid::new_v4(), source, spec, MASTER)
        .await
        .unwrap();
    Probe::begin_create(&db, spec.intent, MASTER).await.unwrap();
    let resource = resource();
    Probe::record_resource(&db, spec.intent, &resource)
        .await
        .unwrap();
    Probe::consume_stdin(&db, spec.intent, &resource, MASTER)
        .await
        .unwrap();
    assert!(Probe::consume_stdin(&db, spec.intent, &resource, MASTER)
        .await
        .is_err());
    assert!(
        Probe::complete(&db, spec.intent, &resource, &tools(), MASTER)
            .await
            .is_err()
    );
    Probe::record_exit(
        &db,
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 7,
            oom_killed: false,
        },
    )
    .await
    .unwrap();
    assert!(
        Probe::complete(&db, spec.intent, &resource, &tools(), MASTER)
            .await
            .is_err()
    );
    assert!(Probe::tools(&db, spec.intent, MASTER).await.is_err());
    db.cleanup().await.unwrap();
}
#[tokio::test]
async fn captured_map_and_exact_removal_receipts_survive_idempotent_retry() {
    let (db, source, spec) = fixture().await;
    let spec = Probe::reserve(&db, Uuid::new_v4(), source, spec, MASTER)
        .await
        .unwrap();
    Probe::begin_create(&db, spec.intent, MASTER).await.unwrap();
    let resource = resource();
    Probe::record_resource(&db, spec.intent, &resource)
        .await
        .unwrap();
    Probe::consume_stdin(&db, spec.intent, &resource, MASTER)
        .await
        .unwrap();
    Probe::record_exit(
        &db,
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await
    .unwrap();
    Probe::complete(&db, spec.intent, &resource, &tools(), MASTER)
        .await
        .unwrap();
    Probe::complete(&db, spec.intent, &resource, &tools(), MASTER)
        .await
        .unwrap();
    Probe::request_stop(&db, spec.intent).await.unwrap();
    assert!(
        Probe::acknowledge_removed(&db, spec.intent, Some(&self::resource()))
            .await
            .is_err()
    );
    Probe::acknowledge_removed(&db, spec.intent, Some(&resource))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(Probe::tools(&db, spec.intent, MASTER).await.unwrap()).unwrap(),
        serde_json::to_value(tools()).unwrap()
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn legacy_verification_has_no_invented_tools_and_distribution_restore_revokes() {
    let (db, mut source, mut spec) = fixture().await;
    source.runtime_config = json!({"interpreter":{"binary":"/usr/bin/owned"}});
    source.runtime_distributions = json!({"verification":{"commands":[{"binary":"/usr/bin/owned","exit_code":7,"pattern":"owned"}]}});
    sqlx::query("UPDATE runtime SET execution_config=$2,distributions=$3 WHERE id=$1")
        .bind(source.runtime)
        .bind(&source.runtime_config)
        .bind(&source.runtime_distributions)
        .execute(&*db)
        .await
        .unwrap();
    let definition = attune_common::runtime_detection::NativeRuntimeVerification {
        registered_version: false,
        execution_config: source.runtime_config.clone(),
        distributions: source.runtime_distributions.clone(),
    };
    spec.invocation.argv = vec![
        format!("{RUNNER_ROOT}/linux-amd64"),
        "inspect-runtime-verification".into(),
        "/usr/bin:/bin".into(),
        serde_json::to_string(&definition).unwrap(),
    ];
    let spec = Probe::reserve(&db, Uuid::new_v4(), source.clone(), spec, MASTER)
        .await
        .unwrap();
    Probe::begin_create(&db, spec.intent, MASTER).await.unwrap();
    let resource = resource();
    Probe::record_resource(&db, spec.intent, &resource)
        .await
        .unwrap();
    Probe::consume_stdin(&db, spec.intent, &resource, MASTER)
        .await
        .unwrap();
    Probe::record_exit(
        &db,
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await
    .unwrap();
    assert!(
        Probe::complete(&db, spec.intent, &resource, &tools(), MASTER)
            .await
            .is_err()
    );
    Probe::complete(&db, spec.intent, &resource, &VerifiedToolMap::new(), MASTER)
        .await
        .unwrap();
    assert!(Probe::tools(&db, spec.intent, MASTER)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE runtime SET distributions='{}' WHERE id=$1")
        .bind(source.runtime)
        .execute(&*db)
        .await
        .unwrap();
    sqlx::query("UPDATE runtime SET distributions=$2 WHERE id=$1")
        .bind(source.runtime)
        .bind(&source.runtime_distributions)
        .execute(&*db)
        .await
        .unwrap();
    assert!(Probe::tools(&db, spec.intent, MASTER).await.is_err());
    db.cleanup().await.unwrap();
}
