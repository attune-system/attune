#![cfg(target_os = "linux")]

mod helpers;

use attune_common::{
    blob_store::{ObjectKey, ProviderVersion, StoredObject},
    child_process_environment::ChildProcessEnvironment,
    models::{PackInstallStatus, WorkerRole, WorkerStatus, WorkerType},
    repositories::{
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::CreateWorkerInput,
        runtime_preparation::{
            can_consume, PreparationPlan, PreparationRequest, RuntimePreparationRepository,
        },
        PackInstallRepository, WorkerRepository,
    },
    runtime_lifecycle::{
        InstalledPreparation, PreparationScope, PreparationTarget, PreparationWorkload,
        RuntimeRecipe, PREPARATION_TRIGGER,
    },
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
    let worker_input = CreateWorkerInput {
        name: "prep-builder".into(),
        worker_type: WorkerType::Local,
        runtime: None,
        host: None,
        port: None,
        status: Some(WorkerStatus::Active),
        capabilities: Some(
            json!({"runtime_preparation_scope": scope, "runtime_recipe_digests": [target.recipe_digest]}),
        ),
        meta: None,
    };
    let incarnation = Uuid::new_v4();
    let worker = WorkerRepository::register_incarnation(
        &database,
        worker_input.clone(),
        WorkerRole::Action,
        incarnation,
    )
    .await
    .unwrap();
    let plan = PreparationPlan {
        requests: vec![PreparationRequest {
            target: target.clone(),
            recipe,
        }],
        scopes: vec![scope.clone()],
    };
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

    let make_attempt = |builder| {
        let repository = &repository;
        let database = &database;
        let pack_ref = &pack.r#ref;
        let candidate = &candidate;
        let plan = &plan;
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
                plan,
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
    let abandoned = make_attempt(worker.id).await;
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
    let worker = WorkerRepository::register_incarnation(
        &database,
        worker_input.clone(),
        WorkerRole::Action,
        replacement,
    )
    .await
    .unwrap();
    assert!(!RuntimePreparationRepository::finish(
        &database,
        abandoned,
        worker.id,
        incarnation,
        &[]
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
        abandoned,
        activation.id,
        &candidate,
        &plan
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let active = PackReleaseRepository::find_active_by_pack_ref(&database, &pack.r#ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.id, original.id);

    let attempt = make_attempt(worker.id).await;
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
    assert!(
        RuntimePreparationRepository::finish(&database, attempt, worker.id, replacement, &[])
            .await
            .is_err()
    );
    // Exercise the trusted worker-to-repository protocol independently of the
    // filesystem and Bubblewrap boundary, which the lifecycle suite covers.
    let ready: InstalledPreparation = serde_json::from_value(json!({
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
    .unwrap();
    assert!(RuntimePreparationRepository::finish(
        &database,
        attempt,
        worker.id,
        replacement,
        &[ready]
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
        attempt,
        activation.id,
        &original,
        &plan
    )
    .await
    .is_err());
    PackReleaseRepository::activate(&mut tx, pack.id, candidate.id)
        .await
        .unwrap();
    RuntimePreparationRepository::bind_release(&mut tx, attempt, activation.id, &candidate, &plan)
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
    let mut different = worker.clone();
    different.capabilities.as_mut().unwrap()["runtime_preparation_scope"] = json!(other);
    assert!(!can_consume(&different, &ready));
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
