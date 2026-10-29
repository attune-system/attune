//! Real initial Wait output on the existing owner/admission/receipt fixture.
use super::*;
use attune_common::repositories::native_wait_materialization::{
    InitialWaitApply, NativeInitialWaitMaterializationRepository as Initial,
    NativeWaitCompletionReplayRepository as Replay,
};

pub(super) async fn complete(
    pool: &sqlx::PgPool,
    workflow: Id,
    parent: Id,
    task: &str,
    target: Target,
    key: &str,
) {
    let (credential, human, pack_ref) = file_producer(pool, parent, key).await;
    let files = tempfile::tempdir().unwrap();
    let store =
        attune_common::blob_store::FilesystemBlobStore::new(files.path().join("objects")).unwrap();
    let mut tx = pool.begin().await.unwrap();
    protect(&mut tx).await;
    status(
        &mut tx,
        target,
        match target {
            Target::Execution(_) => "timeout",
            Target::Inquiry(_) => "cancelled",
            Target::WorkQueueItem(_) => "failed",
        },
    )
    .await;
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    protect(&mut tx).await;
    let children_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent=$1")
        .bind(parent)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut plan = Initial::prepare(&mut tx, workflow, task, Some(key), None)
        .await
        .unwrap()
        .unwrap();
    assert!(Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .is_none());
    let outcome = plan.apply(&mut tx, key).await.unwrap();
    assert!(matches!(outcome, InitialWaitApply::Recovered(children) if children.is_empty()));
    assert_eq!(plan.completions().len(), 1);
    assert_eq!(plan.completions()[0].id, parent);
    assert_eq!(plan.completions()[0].status, ExecutionStatus::Failed);
    let result = plan.completions()[0].result.clone().unwrap();
    let wait = Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .unwrap();
    let release: (Id, Id) = sqlx::query_as("SELECT snapshot,original_snapshot FROM runtime_native_wait_completion_release WHERE execution=$1 AND workflow=$2 AND wait=$3")
        .bind(parent).bind(workflow).bind(wait.id).fetch_one(&mut *tx).await.unwrap();
    assert!(release.0 > release.1 && release.1 > 0);
    let storage = attune_common::repositories::native_input_snapshot::NativeInputSnapshotRepository::storage_evidence(
        &mut tx, release.0).await.unwrap();
    let completion_evidence = storage.wait_completion().unwrap();
    let original_task_path = completion_evidence.original_task_path().to_owned();
    let original_approvals = completion_evidence.original_authority().approval_ids();
    assert!(!original_approvals.is_empty());
    assert_eq!(
        completion_evidence.original_metadata().source_site(),
        Some(completion_evidence.original_source_site()),
        "the source column name must not replace the root JSON pointer"
    );
    let fields = Secrets::find_stored_by_entity(&mut *tx, "execution_result", parent)
        .await
        .unwrap();
    let (ordinary, mut receipts) =
        attune_common::repositories::immutable_input_origin::prepare_input_receipts(
            &mut tx,
            "execution_result",
            parent,
            &fields,
            key,
        )
        .await
        .unwrap();
    assert!(ordinary.is_empty());
    assert_eq!(receipts.len(), 1);
    let receipt = receipts.remove(0);
    let original_kind = match target {
        Target::Execution(_) => "execution_result",
        Target::Inquiry(_) => "inquiry_response",
        Target::WorkQueueItem(_) => "queue_item_result",
    };
    let mut actual = receipt
        .native_source_metadata()
        .iter()
        .map(|metadata| (metadata.source_kind(), metadata.snapshot()))
        .collect::<Vec<_>>();
    let mut expected = vec![
        (original_kind, release.1),
        ("wait_completion_metadata", release.0),
    ];
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "original observation and output keep distinct identities"
    );
    let mut reader_sources =
        attune_common::repositories::native_input_disclosure::NativeWaitSources::default();
    reader_sources
        .prepare_context(&mut tx, &receipt, key, 0)
        .await
        .unwrap();
    let source = reader_sources
        .wait_completion(
            receipt.native_source_metadata(),
            &attune_common::secret_provenance::SecretOrigin::Entity {
                entity_type: "wait_completion_metadata".into(),
                entity_id: release.0,
                path: String::new(),
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(source.original_snapshot(), release.1);
    assert_eq!(source.metadata().snapshot(), release.0);
    reader_sources.ensure_held_on(&mut tx).await.unwrap();
    let children_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent=$1")
        .bind(parent)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        children_after, children_before,
        "no synthetic child or guarded action"
    );
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();

    let held_file = super::super::file_native_retention_cases::hold_wait_output(
        pool,
        &credential,
        human,
        &pack_ref,
        key,
        &receipt,
        files.path(),
        &store,
    )
    .await;
    super::super::file_native_retention_cases::upgrade_held_file(pool, &held_file, &store, key)
        .await;
    let pinned: Vec<Id> = sqlx::query_scalar("SELECT source FROM file_evidence_pin WHERE owner=$1 AND kind='runtime_native_input_snapshot' ORDER BY source")
        .bind(held_file.version).fetch_all(pool).await.unwrap();
    assert!(
        pinned.contains(&release.0) && pinned.contains(&release.1),
        "the file must retain output and original Wait archives independently"
    );

    let mut forbidden = pool.begin().await.unwrap();
    protect(&mut forbidden).await;
    let error = attune_common::repositories::native_input_snapshot::NativeInputSnapshotRepository::prepare_released_wait(
        &mut forbidden, parent, &original_task_path, wait.id, key).await;
    assert!(
        matches!(error, Err(attune_common::Error::PermissionDenied(_))),
        "terminal replay must not enable live input delivery"
    );
    forbidden.rollback().await.unwrap();

    // The owning exclusive collector must really remove the source. Cold replay
    // below therefore cannot pass by rereading a surviving execution/inquiry/item.
    reclaim(pool, target).await;

    let mut tx = pool.begin().await.unwrap();
    protect(&mut tx).await;
    let mut replay = Replay::prepare(&mut tx, &wait, Some(key))
        .await
        .unwrap()
        .unwrap();
    assert!(replay.apply(&mut tx, key).await.unwrap().is_empty());
    assert_eq!(replay.completions().len(), 1);
    assert_eq!(replay.completions()[0].result.as_ref(), Some(&result));
    replay.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    replay.ensure_fresh().unwrap();

    let mut cold = pool.begin().await.unwrap();
    protect(&mut cold).await;
    let fields = Secrets::find_stored_by_entity(&mut *cold, "execution_result", parent)
        .await
        .unwrap();
    let (ordinary, receipts) =
        attune_common::repositories::immutable_input_origin::prepare_input_receipts(
            &mut cold,
            "execution_result",
            parent,
            &fields,
            key,
        )
        .await
        .unwrap();
    assert!(ordinary.is_empty());
    assert_eq!(receipts.len(), 1);
    let mut reader_sources =
        attune_common::repositories::native_input_disclosure::NativeWaitSources::default();
    reader_sources
        .prepare_context(&mut cold, &receipts[0], key, 0)
        .await
        .unwrap();
    reader_sources.ensure_held_on(&mut cold).await.unwrap();
    cold.commit().await.unwrap();

    settle_file(
        pool,
        held_file,
        files.path(),
        &store,
        [release.0, release.1],
    )
    .await;
    files.close().unwrap();

    let mut revoke = pool.begin().await.unwrap();
    protect(&mut revoke).await;
    for approval in original_approvals {
        assert!(attune_common::repositories::runtime_authority::RuntimeAuthorityRepository::revoke_approval(
            &mut revoke, approval).await.unwrap());
    }
    revoke.commit().await.unwrap();
    let mut fresh = pool.begin().await.unwrap();
    protect(&mut fresh).await;
    let rejection = Replay::prepare(&mut fresh, &wait, Some(key)).await;
    assert!(
        matches!(rejection, Err(attune_common::Error::PermissionDenied(_))),
        "fresh replay must reject revocation of the original observer authority"
    );
    fresh.rollback().await.unwrap();
}

pub(super) async fn file_producer(
    pool: &sqlx::PgPool,
    parent: Id,
    key: &str,
) -> (
    attune_common::repositories::runtime_authority::RuntimeCredentialBinding,
    Id,
    String,
) {
    use attune_common::repositories::{
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        runtime_authority::{RuntimeAuthorityRepository as Authority, RuntimeOwner},
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        Update,
    };
    let mut tx = pool.begin().await.unwrap();
    protect(&mut tx).await;
    let root = ExecutionRepository::find_by_id(&mut *tx, parent)
        .await
        .unwrap()
        .unwrap();
    let source = ExecutionRepository::find_by_parent(&mut *tx, parent)
        .await
        .unwrap()
        .into_iter()
        .find(|child| {
            child
                .workflow_task
                .as_ref()
                .is_some_and(|task| task.task_name == "source")
        })
        .unwrap();
    let snapshot = source.executable_snapshot.as_ref().unwrap();
    let admission = Authority::execution_admission(&mut tx, parent)
        .await
        .unwrap()
        .unwrap();
    let policy = Authority::find_policy(&mut tx, admission.policy_revision)
        .await
        .unwrap()
        .unwrap();
    let binding = attune_common::runtime_authority::InvocationBinding::Action {
        action_ref: source.action_ref.clone(),
    };
    let selected = policy
        .policy
        .select(
            policy.id,
            &binding,
            &["standard".into()],
            admission.authority.attribution.clone(),
        )
        .unwrap();
    let producer = ExecutionRepository::create_pinned(
        &mut *tx,
        attune_common::repositories::execution::CreateExecutionInput {
            action: source.action,
            action_ref: source.action_ref.clone(),
            executor: root.executor,
            status: ExecutionStatus::Requested,
            config: None,
            permission_set_refs: selected.selected_refs.clone(),
            ..Default::default()
        },
        snapshot,
    )
    .await
    .unwrap();
    let record =
        Authority::record_admission(&mut tx, RuntimeOwner::Execution(producer.id), &selected)
            .await
            .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, producer.id, &schema)
        .await
        .unwrap();
    Contributors::copy(&mut tx, "execution", parent, "execution", producer.id)
        .await
        .unwrap();
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        producer.id,
        Some(record.id),
        snapshot,
        Some(json!({})),
        &[],
    )
    .await
    .unwrap();
    attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::seal_execution_inputs(
        &mut tx, producer.id, key).await.unwrap();
    ExecutionRepository::update(
        &mut *tx,
        producer.id,
        attune_common::repositories::execution::UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let credential = attune_common::repositories::runtime_authority::RuntimeCredentialBinding {
        execution_id: producer.id,
        admission_id: record.id,
        sponsor_identity_id: record.authority.owner.sponsor_identity_id,
        action_ref: producer.action_ref,
        attempt: record.authority.attempt,
    };
    tx.commit().await.unwrap();
    (
        credential,
        root.executor.unwrap(),
        record.authority.owner.pack_ref.clone(),
    )
}

pub(super) async fn settle_file(
    pool: &sqlx::PgPool,
    held: super::super::file_native_retention_cases::Held,
    directory: &std::path::Path,
    store: &attune_common::blob_store::FilesystemBlobStore,
    archives: [Id; 2],
) {
    use attune_common::{
        blob_store::BlobStore,
        repositories::{
            artifact::ArtifactVersionRepository,
            file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        },
    };
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &[held.version])
            .await
            .unwrap(),
        vec![held.version]
    );
    assert!(ArtifactVersionRepository::delete(&mut *tx, held.version)
        .await
        .unwrap());
    for archive in archives {
        assert!(
            !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", archive)
                .await
                .unwrap()
        );
    }
    tx.commit().await.unwrap();
    store
        .delete(&held.object.key, &held.object.provider_version)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::object_removed(&mut tx, held.object.key.as_str(), Some("v:wrong"))
        .await
        .unwrap();
    for archive in archives {
        assert!(
            !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", archive)
                .await
                .unwrap()
        );
    }
    Evidence::object_removed(
        &mut tx,
        held.object.key.as_str(),
        Some(held.object.provider_version.as_stored()),
    )
    .await
    .unwrap();
    Evidence::staging_removed(&mut tx, "wrong", held.version)
        .await
        .unwrap();
    for archive in archives {
        assert!(
            !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", archive)
                .await
                .unwrap()
        );
    }
    tx.commit().await.unwrap();
    tokio::fs::remove_file(directory.join(&held.staging))
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::staging_removed(&mut tx, &held.staging, held.version)
        .await
        .unwrap();
    let pins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_evidence_pin WHERE owner=$1")
        .bind(held.version)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        pins, 0,
        "only exact object and staging settlement may release file custody"
    );
    tx.commit().await.unwrap();
}
