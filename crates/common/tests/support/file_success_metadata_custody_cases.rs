//! Central-only, UNRUN. H's actual admitted source -> B frame -> A01027
//! publication -> F's real file keeper. No generated status/warning DTO.
use super::*;
#[path = "../native_failure_materialization_tests.rs"]
// Reuse H's owning fixture. Its other repository cases are not invoked here.
#[allow(dead_code)]
pub(crate) mod source;
use attune_common::{
    blob_store::settle_atomic_file_put,
    repositories::{
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        native_failure_materialization::NativeFailureMaterializationRepository as State,
        native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
        workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository as Workflows},
    },
    secret_provenance::SecretProvenance,
    secret_values::{prepare_secret_values, redact_secret_path_sources},
};

#[tokio::test]
async fn success_metadata_generated_default_file_custody_survives_raw_child_gc_and_exact_cleanup() {
    run(
        source::NativeResultCase::DefaultGeneratedSuccess,
        false,
        false,
    )
    .await;
}
#[tokio::test]
async fn success_metadata_canonical_warning_file_custody_retains_real_leaf_receipt_until_exact_cleanup(
) {
    run(
        source::NativeResultCase::GeneratedWarningCandidate,
        true,
        false,
    )
    .await;
}

#[tokio::test]
async fn encryption_upgrade_preserves_nested_success_observations_and_file_custody() {
    run(
        source::NativeResultCase::GeneratedWarningCandidate,
        true,
        true,
    )
    .await;
}

async fn run(case: source::NativeResultCase, warning: bool, compatibility: bool) {
    let (db, files, parent, workflow, child) = source::original_success_file_source(case).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, source::KEY)
        .await
        .unwrap()
        .unwrap();
    let state = State::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
        .await
        .unwrap()
        .unwrap();
    let output = State::prepare_success_output(
        &mut tx,
        parent.id,
        workflow.id,
        child.id,
        &plan,
        source::KEY,
    )
    .await
    .unwrap()
    .unwrap();
    output.require_original_rows(&parent, &workflow).unwrap();
    plan.record(&mut tx, &workflow, source::KEY).await.unwrap();
    let projected = Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            completed_tasks: Some(state.projected_completed_tasks().to_vec()),
            current_tasks: Some(state.projected_current_tasks().to_vec()),
            failed_tasks: Some(state.projected_failed_tasks().to_vec()),
            skipped_tasks: Some(state.projected_skipped_tasks().to_vec()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    state.require_projected_workflow(&projected).unwrap();
    let value = output.value().clone();
    assert_eq!(value.get("output_validation_warning").is_some(), warning);
    // H's actual output consumer invokes A's real canonical publisher. Never
    // call B's legacy output recorder or construct a metadata/status source.
    let (actual, sources) = output
        .record(&mut tx, &projected, source::KEY)
        .await
        .unwrap();
    assert_eq!(actual, value);
    let (public, fields) = redact_secret_path_sources(actual, &sources);
    let fields = prepare_secret_values(fields, source::KEY).unwrap();
    Secrets::replace_entity_with_conn(&mut tx, "execution_result", parent.id, &fields)
        .await
        .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        parent.id,
        UpdateExecutionInput {
            result: Some(public),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let stored = Secrets::find_stored_by_entity(&mut *tx, "execution_result", parent.id)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    let provenance: SecretProvenance =
        serde_json::from_str(stored[0].source_ref.as_deref().unwrap()).unwrap();
    let mut receipt = None;
    let site = attune_common::repositories::inherited_input::InputSite {
        execution: parent.id,
        kind: attune_common::repositories::inherited_input::InputEntityKind::ExecutionResult,
        purpose: attune_common::repositories::inherited_input::InputSitePurpose::Value,
        path: stored[0].json_path.clone(),
    };
    for origin in &provenance.origins {
        if let Ok(actual) = Inputs::receipt_context(&mut tx, &site, origin, source::KEY).await {
            if actual
                .native_source_metadata()
                .iter()
                .any(|metadata| metadata.source_kind() == "ordinary_success_metadata")
            {
                receipt = Some(actual);
                break;
            }
        }
    }
    let receipt = receipt.expect("H must publish A's official ordinary-success VALUE receipt");
    let metadata = receipt
        .native_source_metadata()
        .iter()
        .find(|metadata| metadata.source_kind() == "ordinary_success_metadata")
        .unwrap();
    let snapshot = metadata.snapshot();
    let evidence = Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    let success = evidence.success_metadata().unwrap();
    assert!(
        serde_json::to_value(&parent)
            .unwrap()
            .get("executable_snapshot")
            .is_none(),
        "public execution serialization must not expose the executable snapshot"
    );
    assert!(success.original_parent().executable_snapshot.is_some(),
        "the private success archive must retain its original executable graph after storage round-trip");
    assert_eq!(
        serde_json::to_value(&success.original_parent().executable_snapshot).unwrap(),
        serde_json::to_value(&parent.executable_snapshot).unwrap(),
    );
    assert_eq!(success.completed_execution(), child.id);
    let leaves = success
        .output_leaves()
        .iter()
        .map(|leaf| leaf.receipt())
        .collect::<Vec<_>>();
    assert_eq!(leaves.len(), usize::from(warning));
    let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, parent.id)
        .await
        .unwrap()
        .unwrap();
    let credential = RuntimeCredentialBinding {
        execution_id: parent.id,
        admission_id: admission.id,
        sponsor_identity_id: admission.authority.owner.sponsor_identity_id,
        action_ref: admission.authority.binding.action_ref().into(),
        attempt: admission.authority.attempt,
    };
    assert!(
        admission.authority.has_api_access(),
        "the file producer must opt in before admission"
    );
    assert_eq!(parent.permission_set_refs, vec!["standard"]);
    let pack_ref = admission.authority.owner.pack_ref.clone();
    let terminal = state.projected_status();
    assert_eq!(terminal, ExecutionStatus::Completed);
    output.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    let directory = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(directory.path().join("objects")).unwrap();
    // A's real release already exists. Capture while this admitted writer is
    // Running, then persist the real H-projected terminal state, not a DTO.
    let held = hold_for_producer(
        &db,
        &FileProducer {
            credential: &credential,
            human_id: credential.sponsor_identity_id,
            pack_ref: &pack_ref,
            key: source::KEY,
        },
        &receipt,
        directory.path(),
        &store,
    )
    .await;
    if compatibility {
        let before = read_file_payload(&db, &held, &store, source::KEY).await;
        encryption_compatibility::legacy_upgrade_roundtrip(&db, source::KEY).await;
        assert_eq!(
            read_file_payload(&db, &held, &store, source::KEY).await,
            before
        );
    }
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    ExecutionRepository::update(
        &mut *tx,
        parent.id,
        UpdateExecutionInput {
            status: Some(terminal),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            status: Some(terminal),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(ExecutionRepository::delete(&mut *tx, child.id)
        .await
        .unwrap());
    assert!(
        Secrets::find_by_entity(&mut *tx, "execution_result", child.id)
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
    assert_pins(&db, held.version, snapshot, &leaves).await;

    sqlx::query("UPDATE file_evidence_owner SET evidence_state='pending_audit' WHERE version=$1")
        .bind(held.version)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(Evidence::readmit(db.pool(), held.version).await.unwrap());
    assert_pins(&db, held.version, snapshot, &leaves).await;
    if warning {
        // A01027 still requires live output-leaf receipts. The file keeper must
        // protect them physically; a successful DELETE command may affect zero
        // rows when its pin trigger refuses reclamation.
        let mut tx = db.begin().await.unwrap();
        Evidence::lock_reclamation(&mut tx).await.unwrap();
        sqlx::query("SAVEPOINT leaf_gc_denial")
            .execute(&mut *tx)
            .await
            .unwrap();
        let deletion = sqlx::query("DELETE FROM runtime_input_receipt WHERE id=ANY($1)")
            .bind(&leaves)
            .execute(&mut *tx)
            .await;
        if let Ok(deleted) = deletion {
            assert_eq!(
                deleted.rows_affected(),
                0,
                "file custody must retain every original output leaf"
            );
            let retained = Snapshots::storage_evidence(&mut tx, snapshot)
                .await
                .unwrap();
            assert_eq!(
                retained.success_metadata().unwrap().output_leaves().len(),
                leaves.len()
            );
        }
        sqlx::query("ROLLBACK TO SAVEPOINT leaf_gc_denial")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert_pins(&db, held.version, snapshot, &leaves).await;
    }

    // A genuine unresolved atomic PUT keeps every pin, even after metadata GC.
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    let put = Evidence::begin_put(&mut tx, held.version)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::request_cleanup(&mut tx, &[held.version])
        .await
        .unwrap();
    assert!(ArtifactVersionRepository::delete(&mut *tx, held.version)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    assert_pins(&db, held.version, snapshot, &leaves).await;
    let recovery = Evidence::recovery_candidates(db.pool(), 1).await.unwrap();
    assert_eq!(recovery.len(), 1);
    let settled = settle_atomic_file_put(&store, &recovery[0].key().unwrap(), recovery[0].token())
        .await
        .unwrap();
    assert_pins(&db, held.version, snapshot, &leaves).await;
    Evidence::complete_put_recovery(db.pool(), &recovery[0], &settled)
        .await
        .unwrap();
    assert!(
        Evidence::finish_put(db.pool(), Some(put), Some(&held.object))
            .await
            .is_err()
    );
    assert_pins(&db, held.version, snapshot, &leaves).await;
    tokio::fs::remove_file(directory.path().join(&held.staging))
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::staging_removed(&mut tx, &held.staging, held.version)
        .await
        .unwrap();
    let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1")
        .bind(held.version)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(pins, 0);
    assert!(ExecutionRepository::delete(&mut *tx, parent.id)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    for _ in 0..256 {
        PackRetentionRepository::collect(
            db.pool(),
            directory.path(),
            Utc::now() - Duration::days(30),
            1,
            1,
        )
        .await
        .unwrap();
        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM runtime_native_input_snapshot WHERE id=$1")
                .bind(snapshot)
                .fetch_one(db.pool())
                .await
                .unwrap();
        if left == 0 {
            break;
        }
    }
    let left: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM runtime_native_input_snapshot WHERE id=$1)+(SELECT count(*) FROM runtime_native_success_metadata_observation WHERE snapshot=$1)+(SELECT count(*) FROM runtime_native_success_metadata_release WHERE snapshot=$1)").bind(snapshot).fetch_one(db.pool()).await.unwrap();
    assert_eq!(left, 0);
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

async fn assert_pins(db: &TestDatabase, version: Id, snapshot: Id, leaves: &[Id]) {
    let pinned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_evidence_pin WHERE owner=$1 AND kind='runtime_native_input_snapshot' AND source=$2)").bind(version).bind(snapshot).fetch_one(db.pool()).await.unwrap();
    assert!(pinned);
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_native_success_metadata_release WHERE snapshot=$1",
    )
    .bind(snapshot)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(retained, 1);
    let pinned_leaves: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1 AND kind='runtime_input_receipt' AND source=ANY($2)").bind(version).bind(leaves).fetch_one(db.pool()).await.unwrap();
    assert_eq!(pinned_leaves as usize, leaves.len());
}
