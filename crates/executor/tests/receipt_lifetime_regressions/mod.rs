#[tokio::test]
async fn adhoc_target_under_recorded_parent_cannot_reopen_without_child_admission() {
    use attune_common::repositories::{
        executable_snapshot::ExecutableSnapshotRepository,
        execution_admission::ExecutionAdmissionRepository as Admission,
        runtime_authority::RuntimeOwner,
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let fixture = InquirySchedulerFixture::create().await;
    let pool = fixture.database.pool();
    let pack = PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: "receipt_adhoc_target_it".into(),
            label: "Receipt ad-hoc target".into(),
            description: None,
            version: "1.0.0".into(),
            conf_schema: serde_json::json!({}),
            config: serde_json::json!({}),
            meta: serde_json::json!({}),
            tags: Vec::new(),
            runtime_deps: Vec::new(),
            dependencies: Vec::new(),
            is_standard: false,
            installers: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let action = ActionRepository::create(
        pool,
        CreateActionInput {
            r#ref: "receipt_adhoc_target_it.target".into(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Genuinely ad-hoc negative target".into(),
            description: None,
            entrypoint: "request.sh".into(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: serde_json::json!({}),
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
            param_schema: None,
            out_schema: None,
            is_adhoc: true,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();
    // Own a separate activated ad-hoc code release. Do not relabel a managed
    // executable or add this target to the recorded parent's captured policy.
    let mut activation = pool.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut activation,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "b".repeat(64),
            object_key: "receipt-adhoc-target-it".into(),
            provider_version: "test".into(),
            content_path: "/tmp/opencode/receipt-adhoc-target-it".into(),
            archive_size: 1,
            manifest: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate(&mut activation, pack.id, release.id)
        .await
        .unwrap();
    activation.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let parent_admission = Admission::require(&mut tx, RuntimeOwner::Execution(fixture.parent.id))
        .await
        .expect("negative has an actual admitted parent, not invented source metadata");
    assert_eq!(parent_admission.owner_id, fixture.parent.id);
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    assert!(snapshot.executable.action.is_adhoc);
    // Deliberately construct an unsupported STORAGE boundary with the raw
    // execution repository. Normal Admission::create_workflow_task... would
    // require the child's admission. Do not delete or corrupt an admitted row,
    // relabel a managed snapshot, disable guards, or invent an accepted seal.
    let child = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            parent: Some(fixture.parent.id),
            enforcement: fixture.parent.enforcement,
            executor: fixture.parent.executor,
            config: Some(serde_json::json!({})),
            status: ExecutionStatus::Requested,
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let binding = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, child.id, &binding)
        .await
        .unwrap();
    assert!(Admission::find(&mut tx, RuntimeOwner::Execution(child.id))
        .await
        .unwrap()
        .is_none());
    let pending = PreparedWorkflowInputPublications::prepare_pending_data(
        &child,
        action.id,
        &action.r#ref,
        &fixture.parent,
    )
    .unwrap();
    let mut publications = WorkflowPublications::default();
    publications.push(pending);
    let rejected = PreparedWorkflowInputPublications::prepare(
        &mut tx,
        &publications,
        Some(INQUIRY_SCHEDULER_INPUT_KEY),
    )
    .await;
    tx.rollback().await.unwrap();
    fixture.database.cleanup().await.unwrap();
    let error = match rejected {
        Err(error) => error,
        Ok(_) => {
            panic!("an ad-hoc target flag cannot waive admission required by its recorded parent")
        }
    };
    assert!(
        error
            .to_string()
            .contains("Recorded child input admission is unavailable"),
        "{error}"
    );
}

#[tokio::test]
async fn legitimate_adhoc_reuse_reopens_and_standalone_fanout_rejects_payload_mismatch() {
    use attune_common::repositories::{
        execution_admission::ExecutionAdmissionRepository as Admission,
        identity::{CreatePermissionAssignmentInput, PermissionAssignmentRepository},
        runtime_authority::RuntimeOwner,
    };
    let fixture = AdhocWorkflowDelegationFixture::create().await.unwrap();
    let pool = fixture.database.pool();
    PermissionAssignmentRepository::create(
        pool,
        CreatePermissionAssignmentInput {
            identity: fixture.identity_id,
            permset: fixture.permission_set_id,
        },
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut messages = WorkflowPublications::default();
    ExecutionScheduler::dispatch_workflow_task_with_conn(
        &mut tx,
        &AtomicUsize::new(0),
        &fixture.parent,
        &fixture.workflow_execution_id,
        fixture.graph.get_task("permitted_single").unwrap(),
        &fixture.context,
        None,
        None,
        &mut messages,
        &mut Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1);
    let original = ExecutionRepository::find_by_id(&mut *tx, messages[0].execution_id)
        .await
        .unwrap()
        .unwrap();
    // Re-read the persisted row just as the standalone fanout reuse fast path
    // does. This is not fabricated execution/receipt evidence.
    let reused = ExecutionRepository::find_by_id(&mut *tx, original.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        reused
            .executable_snapshot
            .as_ref()
            .unwrap()
            .executable
            .action
            .is_adhoc
    );
    assert!(Admission::find(&mut tx, RuntimeOwner::Execution(reused.id))
        .await
        .unwrap()
        .is_none());
    assert!(
        Admission::find(&mut tx, RuntimeOwner::Execution(fixture.parent.id))
            .await
            .unwrap()
            .is_none()
    );
    let valid = PreparedWorkflowInputPublications::prepare_pending_data(
        &reused,
        reused.action.unwrap(),
        &reused.action_ref,
        &fixture.parent,
    )
    .expect("standalone reuse factory accepts matching actual row data");
    let mismatch = PreparedWorkflowInputPublications::prepare_pending_data(
        &reused,
        fixture.parent.action.unwrap(),
        &fixture.parent.action_ref,
        &fixture.parent,
    );
    assert!(
        mismatch.is_err(),
        "reused child cannot publish caller-selected different action data"
    );
    let mut matching = WorkflowPublications::default();
    matching.push(valid);
    let accepted = PreparedWorkflowInputPublications::prepare(&mut tx, &matching, None)
        .await
        .expect("separate legitimate human-owned ad-hoc path needs no fake recorded seal or encryption key");
    accepted.ensure_fresh_on(&mut tx).await.unwrap();
    tx.rollback().await.unwrap();
    fixture.cleanup().await.unwrap();
}
