//! The file writer is independently admitted. Terminal roots never regain live
//! worker delivery merely to make a custody fixture pass.
use super::*;
use attune_common::{
    blob_store::{BlobStore, FilesystemBlobStore},
    repositories::{
        artifact::ArtifactVersionRepository,
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        runtime_authority::RuntimeCredentialBinding,
    },
};

pub(super) async fn writer(
    db: &TestDatabase,
    root: &Execution,
    human: Id,
) -> RuntimeCredentialBinding {
    let mut tx = db.begin().await.unwrap();
    let original = RuntimeAuthorityRepository::execution_admission(&mut tx, root.id)
        .await
        .unwrap()
        .unwrap();
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, original.policy_revision)
        .await
        .unwrap()
        .unwrap();
    let selected = policy
        .policy
        .select(
            policy.id,
            &original.authority.binding,
            &["standard".into()],
            original.authority.attribution.clone(),
        )
        .unwrap();
    let executable = root.executable_snapshot.as_ref().unwrap();
    let writer = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: root.action,
            action_ref: root.action_ref.clone(),
            executor: Some(human),
            permission_set_refs: selected.selected_refs.clone(),
            status: ExecutionStatus::Requested,
            ..Default::default()
        },
        executable,
    )
    .await
    .unwrap();
    let admitted = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(writer.id),
        &selected,
    )
    .await
    .unwrap();
    let schema = Schemas::find(&mut tx, BindingOwner::Execution, root.id)
        .await
        .unwrap()
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, writer.id, &schema)
        .await
        .unwrap();
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        writer.id,
        Some(admitted.id),
        executable,
        Some(json!({"items":[],"literal":LITERAL})),
        &[],
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
        &mut tx, "execution", writer.id, human).await.unwrap();
    attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
        &mut tx, writer.id, KEY,
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        writer.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    RuntimeCredentialBinding {
        execution_id: writer.id,
        admission_id: admitted.id,
        sponsor_identity_id: admitted.authority.owner.sponsor_identity_id,
        action_ref: writer.action_ref,
        attempt: admitted.authority.attempt,
    }
}

pub(super) async fn settle(
    db: &TestDatabase,
    held: super::super::file_native_retention_cases::Held,
    directory: &std::path::Path,
    store: &FilesystemBlobStore,
    snapshot: Id,
) {
    let mut tx = db.begin().await.unwrap();
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
    assert!(
        !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshot)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    store
        .delete(&held.object.key, &held.object.provider_version)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::object_removed(&mut tx, held.object.key.as_str(), Some("v:wrong"))
        .await
        .unwrap();
    assert!(
        !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshot)
            .await
            .unwrap()
    );
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
    assert!(
        !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshot)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    tokio::fs::remove_file(directory.join(&held.staging))
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::staging_removed(&mut tx, &held.staging, held.version)
        .await
        .unwrap();
    let pins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_evidence_pin WHERE owner=$1")
        .bind(held.version)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(pins, 0);
    tx.commit().await.unwrap();
}
