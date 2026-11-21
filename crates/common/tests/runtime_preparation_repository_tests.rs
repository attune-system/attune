#![cfg(target_os = "linux")]

mod helpers;

use attune_common::{
    blob_store::{ObjectKey, ProviderVersion, StoredObject},
    child_process_environment::ChildProcessEnvironment,
    models::{PackInstallStatus, WorkerRole, WorkerStatus, WorkerType},
    repositories::{
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::CreateWorkerInput,
        runtime_preparation::{can_consume, PreparationJob, RuntimePreparationRepository},
        PackInstallRepository, WorkerRepository,
    },
    runtime_lifecycle::{
        InstalledPreparation, PreparationScope, PreparationTarget, PreparationWorkload,
        RuntimeRecipe, PREPARATION_TRIGGER,
    },
    scheduling::WorkerPlacement,
    workload_sandbox::WorkloadSandbox,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn activation_only_binds_complete_current_attempts_to_the_exact_release() {
    let database = helpers::create_test_pool().await.unwrap();
    let pack = helpers::PackFixture::new_unique("prepared_release")
        .create(&database)
        .await
        .unwrap();
    let mut tx = database.begin().await.unwrap();
    let release_input = |version: &str, digest: &str| CreatePackReleaseInput {
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        version: version.into(),
        digest: digest.into(),
        content_path: format!("/packs/.releases/{digest}/pack"),
        object_key: format!("packs/blobs/sha256/{digest}.tar.gz"),
        provider_version: format!("e:{digest}"),
        archive_size: 1,
        manifest: json!({}),
    };
    let original =
        PackReleaseRepository::create_or_get(&mut tx, release_input("1.0.0", &"a".repeat(64)))
            .await
            .unwrap();
    let candidate =
        PackReleaseRepository::create_or_get(&mut tx, release_input("1.1.0", &"b".repeat(64)))
            .await
            .unwrap();
    PackReleaseRepository::activate(&mut tx, pack.id, original.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let recipe: RuntimeRecipe = serde_json::from_value(
        serde_json::from_str::<serde_json::Value>(include_str!(
            "../../../tests/fixtures/runtime-services/install_recipe.json"
        ))
        .unwrap()["execution_config"]["lifecycle"]
            .clone(),
    )
    .unwrap();
    let target = PreparationTarget {
        kind: PreparationWorkload::Action,
        reference: format!("{}.run", pack.r#ref),
        source: "actions/run.py".into(),
        runtime_ref: format!("{}.python", pack.r#ref),
        runtime_names: vec!["python".into()],
        recipe_digest: recipe.digest().unwrap(),
    };
    let preparation_environment = ChildProcessEnvironment::from_parent(
        [(
            "ATTUNE_RUNTIME_PREPARATION_REVISION".into(),
            "revision-1".into(),
        )],
        &[],
    )
    .unwrap()
    .with_workload_sandbox(WorkloadSandbox::Bubblewrap);
    let volume = tempfile::tempdir().unwrap();
    let scope = PreparationScope::open(volume.path(), &preparation_environment)
        .unwrap()
        .unwrap();
    let isolated = tempfile::tempdir().unwrap();
    let other = PreparationScope::open(isolated.path(), &preparation_environment)
        .unwrap()
        .unwrap();
    let worker_input = |name: &str, pool: &str, scope: &PreparationScope| CreateWorkerInput {
        name: name.into(),
        worker_type: WorkerType::Local,
        runtime: None,
        host: None,
        port: None,
        status: Some(WorkerStatus::Active),
        capabilities: Some(json!({
            "runtime_preparation_scope": scope,
            "runtime_recipe_digests": [target.recipe_digest],
            "labels": {"pool": pool},
            "taints": [{"key": "dedicated", "effect": "no_schedule"}],
        })),
        meta: None,
    };
    let builder = worker_input("prep-builder", "first", &scope);
    let incarnation = Uuid::new_v4();
    let worker = WorkerRepository::register_incarnation(
        &database,
        builder.clone(),
        WorkerRole::Action,
        incarnation,
    )
    .await
    .unwrap();
    let second_incarnation = Uuid::new_v4();
    let second_worker = WorkerRepository::register_incarnation(
        &database,
        worker_input("prep-second-builder", "second", &other),
        WorkerRole::Action,
        second_incarnation,
    )
    .await
    .unwrap();
    let outsider = WorkerRepository::register_incarnation(
        &database,
        worker_input("prep-outsider", "other", &scope),
        WorkerRole::Action,
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    let placement = |pool| -> WorkerPlacement {
        serde_json::from_value(json!({
            "selector": {"pool": pool},
            "tolerations": [{"key": "dedicated", "operator": "exists", "effect": "no_schedule"}],
            "affinity": {},
        }))
        .unwrap()
    };
    let second_target = PreparationTarget {
        reference: format!("{}.other", pack.r#ref),
        source: "actions/other.py".into(),
        ..target.clone()
    };
    let jobs = [
        PreparationJob {
            target: target.clone(),
            recipe: recipe.clone(),
            scopes: vec![scope.clone()],
            placements: vec![placement("first")],
        },
        PreparationJob {
            target: second_target.clone(),
            recipe,
            scopes: vec![other.clone()],
            placements: vec![placement("second")],
        },
    ];
    let repository = PackInstallRepository::new(database.pool().clone());
    let activation = repository
        .create(
            &pack.r#ref,
            &candidate.version,
            "update",
            Some(pack.id),
            None,
        )
        .await
        .unwrap();
    assert!(repository
        .begin_activation(activation.id)
        .await
        .unwrap()
        .is_some());

    let make_attempt = |builder, job_index: usize| {
        let repository = &repository;
        let database = &database;
        let pack_ref = &pack.r#ref;
        let candidate = &candidate;
        let job = &jobs[job_index];
        async move {
            let attempt = repository
                .create(
                    pack_ref,
                    &candidate.version,
                    PREPARATION_TRIGGER,
                    None,
                    None,
                )
                .await
                .unwrap();
            let stored = StoredObject {
                key: ObjectKey::new(candidate.object_key.as_deref().unwrap()).unwrap(),
                provider_version: ProviderVersion::from_stored(
                    candidate.provider_version.as_deref().unwrap(),
                )
                .unwrap(),
                size: 1,
                sha256: [0xbb; 32],
            };
            let mut tx = database.begin().await.unwrap();
            assert!(
                PackInstallRepository::attach_candidate_object(&mut tx, attempt.id, &stored)
                    .await
                    .unwrap()
            );
            RuntimePreparationRepository::create_attempt(
                &mut tx,
                attempt.id,
                activation.id,
                &candidate.digest,
                job,
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            assert!(repository
                .claim_worker(attempt.id, builder, Some("fixture-attempt-secret"))
                .await
                .unwrap()
                .is_some());
            attempt.id
        }
    };
    let fake_ready =
        |target: &PreparationTarget, scope: &PreparationScope| -> InstalledPreparation {
            // Test the repository fence independently of the filesystem. The
            // Bubblewrap lifecycle test exercises real seals and retained trees.
            serde_json::from_value(json!({
                "target": target,
                "scope": scope,
                "release_digest": candidate.digest,
                "directory": Uuid::new_v4(),
                "generation": "generation-complete",
                "seal": Uuid::new_v4(),
                "retained": ["fixture-pack", "fixture-build"],
                "tools": {},
                "tool_hashes": {},
            }))
            .unwrap()
        };
    let ready = fake_ready(&target, &scope);
    let second_ready = fake_ready(&second_target, &other);
    let abandoned = make_attempt(worker.id, 0).await;
    let dispatch = RuntimePreparationRepository::dispatch_job(&database, abandoned)
        .await
        .unwrap()
        .unwrap();
    assert!(
        dispatch.can_build(&worker),
        "the declared taint toleration must be honored"
    );
    assert!(
        !dispatch.can_build(&outsider),
        "sharing storage does not satisfy builder placement"
    );
    assert!(!dispatch.can_build(&second_worker));
    assert!(RuntimePreparationRepository::begin(
        &database,
        abandoned,
        worker.id,
        incarnation,
        &other
    )
    .await
    .unwrap()
    .is_none());
    assert!(RuntimePreparationRepository::begin(
        &database,
        abandoned,
        worker.id,
        incarnation,
        &scope
    )
    .await
    .unwrap()
    .is_some());
    assert!(RuntimePreparationRepository::begin(
        &database,
        abandoned,
        worker.id,
        incarnation,
        &scope
    )
    .await
    .unwrap()
    .is_none());

    let replacement = Uuid::new_v4();
    let worker =
        WorkerRepository::register_incarnation(&database, builder, WorkerRole::Action, replacement)
            .await
            .unwrap();
    assert!(!RuntimePreparationRepository::finish(
        &database,
        abandoned,
        worker.id,
        incarnation,
        &ready
    )
    .await
    .unwrap());
    repository
        .finish_active(abandoned, PackInstallStatus::Failed, None, None, None)
        .await
        .unwrap();
    let mut tx = database.begin().await.unwrap();
    PackReleaseRepository::activate(&mut tx, pack.id, candidate.id)
        .await
        .unwrap();
    assert!(RuntimePreparationRepository::bind_release(
        &mut tx,
        &[abandoned],
        activation.id,
        &candidate,
        &jobs[..1]
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let active = PackReleaseRepository::find_active_by_pack_ref(&database, &pack.r#ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.id, original.id);

    let attempt = make_attempt(worker.id, 0).await;
    let second_attempt = make_attempt(second_worker.id, 1).await;
    assert!(RuntimePreparationRepository::begin(
        &database,
        attempt,
        worker.id,
        replacement,
        &scope
    )
    .await
    .unwrap()
    .is_some());
    assert!(RuntimePreparationRepository::finish(
        &database,
        attempt,
        worker.id,
        replacement,
        &second_ready,
    )
    .await
    .is_err());
    assert!(RuntimePreparationRepository::finish(
        &database,
        attempt,
        worker.id,
        replacement,
        &ready,
    )
    .await
    .unwrap());

    let mut tx = database.begin().await.unwrap();
    PackReleaseRepository::activate(&mut tx, pack.id, candidate.id)
        .await
        .unwrap();
    assert!(RuntimePreparationRepository::bind_release(
        &mut tx,
        &[attempt, second_attempt],
        activation.id,
        &candidate,
        &jobs,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    assert_eq!(
        PackReleaseRepository::find_active_by_pack_ref(&database, &pack.r#ref)
            .await
            .unwrap()
            .unwrap()
            .id,
        original.id
    );

    assert!(RuntimePreparationRepository::begin(
        &database,
        second_attempt,
        second_worker.id,
        second_incarnation,
        &other,
    )
    .await
    .unwrap()
    .is_some());
    assert!(RuntimePreparationRepository::finish(
        &database,
        second_attempt,
        second_worker.id,
        second_incarnation,
        &second_ready,
    )
    .await
    .unwrap());
    assert!(RuntimePreparationRepository::ready_for(
        &*database,
        candidate.id,
        &candidate.digest,
        &target
    )
    .await
    .unwrap()
    .is_empty());
    let mut tx = database.begin().await.unwrap();
    assert!(RuntimePreparationRepository::bind_release(
        &mut tx,
        &[attempt, second_attempt],
        activation.id,
        &original,
        &jobs
    )
    .await
    .is_err());
    PackReleaseRepository::activate(&mut tx, pack.id, candidate.id)
        .await
        .unwrap();
    RuntimePreparationRepository::bind_release(
        &mut tx,
        &[attempt, second_attempt],
        activation.id,
        &candidate,
        &jobs,
    )
    .await
    .unwrap();
    PackInstallRepository::finish_activation_in_transaction(&mut tx, activation.id, pack.id)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let ready = RuntimePreparationRepository::ready_for(
        &*database,
        candidate.id,
        &candidate.digest,
        &target,
    )
    .await
    .unwrap();
    assert_eq!(ready.len(), 1);
    assert!(can_consume(&worker, &ready));
    assert!(!can_consume(&second_worker, &ready));
    let second = RuntimePreparationRepository::ready_for(
        &*database,
        candidate.id,
        &candidate.digest,
        &second_target,
    )
    .await
    .unwrap();
    assert_eq!(second.len(), 1);
    assert!(can_consume(&second_worker, &second));
    assert!(!can_consume(&worker, &second));
    let mut different_entrypoint = target.clone();
    different_entrypoint.source = "actions/changed.py".into();
    assert!(RuntimePreparationRepository::ready_for(
        &*database,
        candidate.id,
        &candidate.digest,
        &different_entrypoint
    )
    .await
    .unwrap()
    .is_empty());
    database.cleanup().await.unwrap();
}
