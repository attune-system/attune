//! Ordinary-only empty handoff regressions. These do not finalize a root.
use super::*;

async fn input_graph_fixture(count: usize) -> (TestDatabase, Execution) {
    let (db, root, _, _, _) = setup_graph_contract(
        json!([]),
        None,
        false,
        0,
        false,
        Some((false, false)),
        false,
        EmptyProgram::InputGraphBudget(count),
    )
    .await;
    (db, root)
}

#[tokio::test]
async fn empty_ordinary_budget_literal_metadata_small_input_graph_succeeds() {
    let (db, root) = input_graph_fixture(1).await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let mut plan = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    let children = plan.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].timeout_seconds, Some(17));
    assert_eq!(
        children[0].worker_selector,
        Some(json!({"zone":"literal-placement"}))
    );
    assert_eq!(
        children[0].trace_tag.as_deref(),
        Some("literal-budget-trace")
    );
    assert_eq!(
        restore(&mut tx, &children[0]).await,
        json!({"payload":{"selected":[],"saved":LITERAL},"ordinal":0})
    );
    let controls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_native_empty_task_control WHERE execution=$1",
    )
    .bind(root.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        controls, 0,
        "this fixture has no dynamic metadata CONTROL receipts"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_ordinary_budget_literal_metadata_input_graphs_exceed_activation_total() {
    let (db, root) = input_graph_fixture(12).await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let result = Iteration::prepare_empty_activation(&mut tx, source, KEY).await;
    assert!(matches!(result, Err(attune_common::Error::PermissionDenied(_))), "input-only source ancestry must count even when child configs are small and all metadata is literal");
    let controls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_native_empty_task_control WHERE execution=$1",
    )
    .bind(root.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(controls, 0);
    let dispatched: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow_task_dispatch WHERE execution_id IN (SELECT id FROM execution WHERE parent=$1)").bind(root.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(dispatched, 0);
    tx.rollback().await.unwrap();
    assert!(ExecutionRepository::find_by_parent(&*db, root.id)
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await.unwrap();
}

async fn dynamic_fixture(
    expires: chrono::DateTime<Utc>,
) -> (TestDatabase, Execution, WorkflowExecution, Id) {
    dynamic_fixture_kind(expires, false).await
}

async fn dynamic_fixture_kind(
    expires: chrono::DateTime<Utc>,
    unconditional: bool,
) -> (TestDatabase, Execution, WorkflowExecution, Id) {
    let lifetime = expires - Utc::now();
    let (db, root, workflow, human, _) = setup_graph_contract(
        json!([]),
        None,
        false,
        0,
        false,
        Some((false, false)),
        false,
        if unconditional {
            EmptyProgram::UnconditionalOrdinarySuccessor
        } else {
            EmptyProgram::DynamicOrdinarySuccessor
        },
    )
    .await;
    let grants = json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.ordinary_timeout"]}}]);
    let set = PermissionSetRepository::create(
        db.pool(),
        CreatePermissionSetInput {
            r#ref: "fixture.ordinary_metadata".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: grants.clone(),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        db.pool(),
        CreatePermissionAssignmentInput {
            identity: human,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let bindings = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let binding = bindings
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/1/timeout")
        .unwrap();
    let expires = Utc::now() + lifetime;
    let approval = Inputs::approve_external_input(
        &mut tx,
        root.id,
        &binding.declaration_revision,
        &binding.source_path,
        &binding.expression,
        human,
        serde_json::from_value(grants).unwrap(),
        Some(expires),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (db, root, workflow, approval)
}

async fn begin(db: &TestDatabase) -> sqlx::Transaction<'_, sqlx::Postgres> {
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    tx
}

async fn restore(conn: &mut sqlx::PgConnection, child: &Execution) -> Value {
    Native::capture_with_key(conn, child.id, &[], Some(KEY))
        .await
        .unwrap()
        .restore_prepared(
            conn,
            ENTITY_EXECUTION_CONFIG,
            child.id,
            child.config.clone().unwrap(),
            Some(KEY),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn empty_ordinary_hardening_dynamic_metadata_stages_publishes_and_cold_replays() {
    dynamic_positive(false).await;
    dynamic_positive(true).await;
}

async fn dynamic_positive(unconditional: bool) {
    let expires = Utc::now() + Duration::hours(1);
    let (db, root, workflow, approval) = dynamic_fixture_kind(expires, unconditional).await;
    let expires: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT expires_at FROM runtime_external_approval WHERE id=$1")
            .bind(approval)
            .fetch_one(&*db)
            .await
            .unwrap();
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let source_snapshot = source.empty_outcome().unwrap().snapshot();
    let mut plan = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    assert_eq!(plan.held_deadline().unwrap(), Some(expires));
    let (receipt, path): (Id, String) = sqlx::query_as("SELECT r.id,r.destination_path FROM runtime_input_receipt r JOIN runtime_input_receipt_staged s ON s.receipt=r.id WHERE r.execution=$1 AND r.destination_path='/workflow/tasks/1/timeout' AND s.publication_transaction IS NULL")
        .bind(root.id).fetch_one(&mut *tx).await.unwrap();
    let site = attune_common::repositories::inherited_input::InputSite {
        execution: root.id,
        kind: attune_common::repositories::inherited_input::InputEntityKind::WorkflowVariables,
        purpose: attune_common::repositories::inherited_input::InputSitePurpose::Control,
        path: path.clone(),
    };
    let origin = attune_common::secret_provenance::SecretOrigin::Entity {
        entity_type: "declared_input:workflow_variables:control".into(),
        entity_id: receipt,
        path,
    };
    assert!(matches!(
        Inputs::receipt_context(&mut tx, &site, &origin, KEY).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    // Neither the generic marker nor a forged root-as-child binding can publish
    // the private metadata. Preserve the genuine issuing plan after each denial.
    for statement in [
        "UPDATE runtime_input_receipt_staged SET publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1",
        "UPDATE runtime_native_empty_task_control SET child=$2,publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1",
    ] {
        sqlx::query("SAVEPOINT metadata_publication_denial").execute(&mut *tx).await.unwrap();
        let query = sqlx::query(statement).bind(receipt);
        let result = if statement.contains("$2") { query.bind(root.id).execute(&mut *tx).await } else { query.execute(&mut *tx).await };
        assert!(matches!(result, Err(sqlx::Error::Database(ref error)) if error.code().as_deref()==Some("P0001")));
        sqlx::query("ROLLBACK TO SAVEPOINT metadata_publication_denial").execute(&mut *tx).await.unwrap();
        sqlx::query("RELEASE SAVEPOINT metadata_publication_denial").execute(&mut *tx).await.unwrap();
    }
    let children = plan.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(children.len(), 1);
    let binding: (Id, Id, bool) = sqlx::query_as("SELECT source_snapshot,child,publication_transaction=prepared_transaction FROM runtime_native_empty_task_control WHERE receipt=$1").bind(receipt).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(binding, (source_snapshot, children[0].id, true));
    let iteration_control: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_native_iteration_control WHERE receipt=$1)",
    )
    .bind(receipt)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(
        !iteration_control,
        "metadata is not a fabricated list control"
    );
    assert!(children[0]
        .workflow_task
        .as_ref()
        .unwrap()
        .task_index
        .is_none());
    assert!(children[0]
        .workflow_task
        .as_ref()
        .unwrap()
        .task_batch
        .is_none());
    assert_eq!(children[0].timeout_seconds, Some(17));
    assert_eq!(
        children[0].workflow_task.as_ref().unwrap().timeout_seconds,
        Some(17)
    );
    assert_eq!(
        restore(&mut tx, &children[0]).await,
        json!({"payload":{"selected":[],"saved":LITERAL},"ordinal":0})
    );
    Inputs::receipt_context(&mut tx, &site, &origin, KEY)
        .await
        .unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    let key_id: Id = sqlx::query_scalar("SELECT id FROM key WHERE ref='system.ordinary_timeout'")
        .fetch_one(&*db)
        .await
        .unwrap();
    attune_common::repositories::key::KeyRepository::update(
        &*db,
        key_id,
        attune_common::repositories::key::UpdateKeyInput {
            value: Some(json!("999")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = begin(&db).await;
    let mut recovery = Iteration::recover_empty_activation(&mut tx, &root, "ordinary", KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovery.held_deadline().unwrap(), Some(expires));
    let recovered = recovery.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(
        recovered[0].timeout_seconds,
        Some(17),
        "recovery must not rerender changed metadata"
    );
    assert_eq!(
        recovered.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![children[0].id]
    );
    assert_eq!(
        restore(&mut tx, &recovered[0]).await,
        json!({"payload":{"selected":[],"saved":LITERAL},"ordinal":0})
    );
    assert_eq!(
        Workflows::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .current_tasks,
        vec!["ordinary_success"]
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_ordinary_hardening_dynamic_metadata_revocation_blocks_preparation() {
    let (db, root, _, approval) = dynamic_fixture(Utc::now() + Duration::hours(1)).await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    sqlx::query("UPDATE runtime_external_approval SET revoked_at=clock_timestamp() WHERE id=$1")
        .bind(approval)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(matches!(
        Iteration::prepare_empty_activation(&mut tx, source, KEY).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    tx.rollback().await.unwrap();
    assert!(ExecutionRepository::find_by_parent(&*db, root.id)
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_ordinary_hardening_dynamic_metadata_expiry_blocks_held_apply() {
    let expires = Utc::now() + Duration::seconds(12);
    let (db, root, _, approval) = dynamic_fixture(expires).await;
    let expires: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT expires_at FROM runtime_external_approval WHERE id=$1")
            .bind(approval)
            .fetch_one(&*db)
            .await
            .unwrap();
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let mut plan = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    assert_eq!(plan.held_deadline().unwrap(), Some(expires));
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT expires_at<=clock_timestamp() FROM runtime_external_approval WHERE id=$1",
            )
            .bind(approval)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            if expired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        plan.apply(&mut tx, KEY).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn dispatch_negative(tamper: bool) {
    let (db, root, workflow, _) = dynamic_fixture(Utc::now() + Duration::hours(1)).await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let mut plan = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    let children = plan.apply(&mut tx, KEY).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin(&db).await;
    if tamper {
        sqlx::query("UPDATE workflow_task_dispatch SET task_name='wrong' WHERE workflow_execution=$1 AND execution_id=$2").bind(workflow.id).bind(children[0].id).execute(&mut *tx).await.unwrap();
    } else {
        sqlx::query(
            "DELETE FROM workflow_task_dispatch WHERE workflow_execution=$1 AND execution_id=$2",
        )
        .bind(workflow.id)
        .bind(children[0].id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    assert!(matches!(
        Iteration::recover_empty_activation(&mut tx, &root, "ordinary", KEY).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    assert_eq!(
        ExecutionRepository::find_by_parent(&mut *tx, root.id)
            .await
            .unwrap()
            .len(),
        1
    );
    tx.rollback().await.unwrap();
    let mut tx = begin(&db).await;
    if tamper {
        let mut metadata = children[0].workflow_task.clone().unwrap();
        metadata.triggered_by = Some("different_original_source".into());
        ExecutionRepository::update(
            &mut *tx,
            children[0].id,
            UpdateExecutionInput {
                workflow_task: Some(metadata),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    } else {
        sqlx::query(
            "DELETE FROM workflow_task_dispatch WHERE workflow_execution=$1 AND execution_id=$2",
        )
        .bind(workflow.id)
        .bind(children[0].id)
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(ExecutionRepository::delete(&mut *tx, children[0].id)
            .await
            .unwrap());
    }
    assert!(
        matches!(
            Iteration::recover_empty_activation(&mut tx, &root, "ordinary", KEY).await,
            Err(attune_common::Error::PermissionDenied(_))
        ),
        "original selection inventory must survive complete dispatch loss or changed attribution"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_ordinary_hardening_missing_dispatch_denies_without_replacement() {
    dispatch_negative(false).await;
}
#[tokio::test]
async fn empty_ordinary_hardening_tampered_dispatch_denies_without_replacement() {
    dispatch_negative(true).await;
}

#[tokio::test]
async fn empty_ordinary_hardening_cold_recovery_rechecks_original_metadata_approval() {
    let (db, root, _, approval) = dynamic_fixture(Utc::now() + Duration::hours(1)).await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let mut plan = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    let children = plan.apply(&mut tx, KEY).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin(&db).await;
    sqlx::query("UPDATE runtime_external_approval SET revoked_at=clock_timestamp() WHERE id=$1")
        .bind(approval)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(matches!(
        Iteration::recover_empty_activation(&mut tx, &root, "ordinary", KEY).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    assert_eq!(
        ExecutionRepository::find_by_parent(&mut *tx, root.id)
            .await
            .unwrap()[0]
            .id,
        children[0].id
    );
    tx.rollback().await.unwrap();
    let mut tx = begin(&db).await;
    Secrets::replace_entity_with_conn(&mut tx, ENTITY_EXECUTION_CONFIG, children[0].id, &[])
        .await
        .unwrap();
    assert!(
        matches!(
            Iteration::recover_empty_activation(&mut tx, &root, "ordinary", KEY).await,
            Err(attune_common::Error::PermissionDenied(_))
        ),
        "missing original first-delivery buffers must deny cold recovery"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn invalid_input(expression: &'static str) {
    let (db, root, _, _, _) = setup_graph_contract(
        json!([]),
        None,
        false,
        0,
        false,
        Some((false, false)),
        false,
        EmptyProgram::InvalidOrdinaryInput(expression),
    )
    .await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    assert!(matches!(
        Iteration::prepare_empty_activation(&mut tx, source, KEY).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    tx.rollback().await.unwrap();
    assert!(ExecutionRepository::find_by_parent(&*db, root.id)
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_ordinary_hardening_child_has_no_invented_item() {
    invalid_input("{{ item }}").await;
}

#[tokio::test]
async fn empty_ordinary_hardening_child_cannot_read_empty_result_value() {
    invalid_input("{{ result() }}").await;
    let (db, root, _, _, _) = setup_graph_contract(
        json!([]),
        None,
        false,
        0,
        false,
        Some((false, false)),
        false,
        EmptyProgram::BudgetOrdinarySuccessor,
    )
    .await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    assert!(matches!(
        Iteration::prepare_empty_activation(&mut tx, source, KEY).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    let staged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_native_empty_task_control WHERE execution=$1",
    )
    .bind(root.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(staged > 1 && staged < 128, "individually valid metadata controls must hit their combined source-graph byte budget, staged={staged}");
    assert!(ExecutionRepository::find_by_parent(&mut *tx, root.id)
        .await
        .unwrap()
        .is_empty());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_ordinary_hardening_recovery_rejects_dispatch_changed_after_preparation() {
    let (db, root, workflow, _) = dynamic_fixture(Utc::now() + Duration::hours(1)).await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let mut plan = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    let children = plan.apply(&mut tx, KEY).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin(&db).await;
    let mut recovery = Iteration::recover_empty_activation(&mut tx, &root, "ordinary", KEY)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "DELETE FROM workflow_task_dispatch WHERE workflow_execution=$1 AND execution_id=$2",
    )
    .bind(workflow.id)
    .bind(children[0].id)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(matches!(
        recovery.apply(&mut tx, KEY).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn file_producer(
    db: &TestDatabase,
    root: &Execution,
) -> attune_common::repositories::runtime_authority::RuntimeCredentialBinding {
    let mut tx = begin(db).await;
    let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, root.id)
        .await
        .unwrap()
        .unwrap();
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, admission.policy_revision)
        .await
        .unwrap()
        .unwrap();
    let selected = policy
        .policy
        .select(
            policy.id,
            &InvocationBinding::Action {
                action_ref: root.action_ref.clone(),
            },
            &["standard".into()],
            admission.authority.attribution.clone(),
        )
        .unwrap();
    let snapshot = root.executable_snapshot.as_ref().unwrap();
    let producer = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: root.action,
            action_ref: root.action_ref.clone(),
            executor: root.executor,
            status: ExecutionStatus::Requested,
            permission_set_refs: selected.selected_refs.clone(),
            ..Default::default()
        },
        snapshot,
    )
    .await
    .unwrap();
    let record = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(producer.id),
        &selected,
    )
    .await
    .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, producer.id, &schema)
        .await
        .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::copy(
        &mut tx,
        "execution",
        root.id,
        "execution",
        producer.id,
    )
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
    Materialize::seal_execution_inputs(&mut tx, producer.id, KEY)
        .await
        .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        producer.id,
        UpdateExecutionInput {
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
    credential
}

#[tokio::test]
async fn empty_ordinary_hardening_input_file_custody_requires_exact_physical_settlement() {
    use attune_common::{
        blob_store::{BlobStore, FilesystemBlobStore},
        repositories::{
            artifact::ArtifactVersionRepository,
            file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        },
    };
    let (db, root, _, _) = dynamic_fixture(Utc::now() + Duration::hours(1)).await;
    let credential = file_producer(&db, &root).await;
    let mut tx = begin(&db).await;
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let original_snapshot = source.empty_outcome().unwrap().snapshot();
    let mut plan = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    let children = plan.apply(&mut tx, KEY).await.unwrap();
    let receipts = Materialize::prepare_execution_input_receipts(&mut tx, children[0].id, KEY)
        .await
        .unwrap();
    assert!(!receipts.is_empty());
    tx.commit().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(directory.path().join("objects")).unwrap();
    let held = super::super::file_native_retention_cases::hold_wait_output(
        db.pool(),
        &credential,
        root.executor.unwrap(),
        &root
            .executable_snapshot
            .as_ref()
            .unwrap()
            .executable
            .action
            .pack_ref,
        KEY,
        &receipts[0],
        directory.path(),
        &store,
    )
    .await;
    let pins: Vec<Id> = sqlx::query_scalar("SELECT source FROM file_evidence_pin WHERE owner=$1 AND kind='runtime_native_input_snapshot'").bind(held.version).fetch_all(&*db).await.unwrap();
    assert!(
        pins.contains(&original_snapshot),
        "ordinary input publication must pin its original empty CONTROL archive"
    );
    let mut tx = begin(&db).await;
    let mut recovery = Iteration::recover_empty_activation(&mut tx, &root, "ordinary", KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        recovery.apply(&mut tx, KEY).await.unwrap()[0].id,
        children[0].id
    );
    assert_eq!(
        restore(&mut tx, &children[0]).await,
        json!({"payload":{"selected":[],"saved":LITERAL},"ordinal":0})
    );
    tx.rollback().await.unwrap();
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
        !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", original_snapshot)
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
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1")
        .bind(held.version)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(count > 0);
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
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1")
        .bind(held.version)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(count > 0);
    tx.commit().await.unwrap();
    tokio::fs::remove_file(directory.path().join(&held.staging))
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::staging_removed(&mut tx, &held.staging, held.version)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1")
        .bind(held.version)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 0);
    tx.commit().await.unwrap();
    directory.close().unwrap();
    db.cleanup().await.unwrap();
}
