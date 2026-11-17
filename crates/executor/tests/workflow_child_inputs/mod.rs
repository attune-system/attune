async fn delegated_child_publications(
    fixture: &WorkflowDelegationFixture,
    conn: &mut PgConnection,
) -> WorkflowPublications {
    CacheEntryRepository::protect_transaction(conn, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut pending = WorkflowPublications::default();
    ExecutionScheduler::dispatch_workflow_task_with_conn(
        conn,
        &AtomicUsize::new(0),
        &fixture.parent,
        &fixture.workflow_execution_id,
        fixture.graph.get_task("permitted_single").unwrap(),
        &fixture.context,
        None,
        None,
        &mut pending,
        &mut Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(pending.len(), 1);
    pending
}

#[tokio::test]
async fn managed_delegated_child_inputs_reopen_before_commit_and_on_reuse() {
    use attune_common::repositories::{
        delegated_execution::DelegatedExecutionRepository as Delegated,
        execution_admission::ExecutionAdmissionRepository as Admission,
        execution_runtime_admission::ExistingExecutionAuthority, runtime_authority::RuntimeOwner,
    };
    let fixture = WorkflowDelegationFixture::create_managed().await.unwrap();
    let mut tx = fixture.database.begin().await.unwrap();
    let pending = delegated_child_publications(&fixture, &mut tx).await;
    let child_id = pending[0].execution_id;
    let child = ExecutionRepository::find_by_id(&mut *tx, child_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !child
            .executable_snapshot
            .as_ref()
            .unwrap()
            .executable
            .action
            .is_adhoc
    );
    assert_eq!(
        Admission::existing_authority(&mut tx, child_id)
            .await
            .unwrap(),
        ExistingExecutionAuthority::Delegated
    );
    assert!(Delegated::find(&mut tx, child_id).await.unwrap().explicit);
    assert!(Admission::find(&mut tx, RuntimeOwner::Execution(child_id))
        .await
        .unwrap()
        .is_none());
    let prepared = PreparedWorkflowInputPublications::prepare(&mut tx, &pending, None)
        .await
        .expect("managed delegated inputs need no recorded receipt");
    prepared.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    prepared.ensure_child_fresh(child_id).unwrap();

    let mut reuse = fixture.database.begin().await.unwrap();
    let mut duplicates = pending.clone();
    duplicates.push(pending[0].clone());
    let prepared = PreparedWorkflowInputPublications::prepare(&mut reuse, &duplicates, None)
        .await
        .unwrap();
    prepared.ensure_fresh_on(&mut reuse).await.unwrap();
    let mut deferred = WorkflowPublications::default();
    deferred.register_created(child_id);
    PreparedWorkflowInputPublications::prepare(&mut reuse, &deferred, None)
        .await
        .unwrap();
    let mut forged = pending[0].clone();
    forged.action_ref.push_str(".different");
    duplicates.push(forged);
    assert!(
        PreparedWorkflowInputPublications::prepare(&mut reuse, &duplicates, None)
            .await
            .is_err()
    );
    reuse.rollback().await.unwrap();
    fixture.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_delegated_input_reopening_rejects_revoked_owner_and_closed_parent() {
    use attune_common::repositories::delegated_execution::DelegatedExecutionRepository as Delegated;
    let fixture = WorkflowDelegationFixture::create_managed().await.unwrap();
    let mut tx = fixture.database.begin().await.unwrap();
    let pending = delegated_child_publications(&fixture, &mut tx).await;
    // This mutation is confined to this test's physical database and transaction.
    sqlx::query("DELETE FROM permission_assignment WHERE identity=$1 AND permset=$2")
        .bind(fixture.identity_id)
        .bind(fixture.permission_set_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        PreparedWorkflowInputPublications::prepare(&mut tx, &pending, None)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = fixture.database.begin().await.unwrap();
    let pending = delegated_child_publications(&fixture, &mut tx).await;
    Delegated::close(&mut tx, fixture.parent.id).await.unwrap();
    assert!(
        PreparedWorkflowInputPublications::prepare(&mut tx, &pending, None)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    fixture.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_delegated_input_reopening_rejects_unavailable_selection_and_wrong_lineage() {
    let fixture = WorkflowDelegationFixture::create_managed().await.unwrap();
    let mut tx = fixture.database.begin().await.unwrap();
    let pending = delegated_child_publications(&fixture, &mut tx).await;
    let mut child = ExecutionRepository::find_by_id(&mut *tx, pending[0].execution_id)
        .await
        .unwrap()
        .unwrap();
    child.workflow_task.as_mut().unwrap().workflow_execution = i64::MAX;
    ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            workflow_task: child.workflow_task,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        PreparedWorkflowInputPublications::prepare(&mut tx, &pending, None)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = fixture.database.begin().await.unwrap();
    let pending = delegated_child_publications(&fixture, &mut tx).await;
    // Simulate missing retained authority, never a recorded-to-delegated repair.
    sqlx::query("DELETE FROM execution_authority_selection WHERE execution=$1")
        .bind(pending[0].execution_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        PreparedWorkflowInputPublications::prepare(&mut tx, &pending, None)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    fixture.cleanup().await.unwrap();
}

#[derive(Clone, Copy)]
enum AdvanceFailureBoundary {
    InputValidation,
    Database,
    Publication,
}

struct AdvanceFailureObserver(AdvanceFailureBoundary);

#[async_trait::async_trait]
impl WorkflowInputCommitObserver for AdvanceFailureObserver {
    async fn after_input_mutations(
        &self,
        _conn: &mut PgConnection,
        publications: &WorkflowPublications,
    ) -> Result<()> {
        assert_eq!(publications.len(), 1, "actual successor was prepared");
        match self.0 {
            AdvanceFailureBoundary::InputValidation => Err(attune_common::Error::PermissionDenied(
                "synthetic private diagnostic must not be copied".into(),
            )
            .into()),
            AdvanceFailureBoundary::Database => {
                Err(attune_common::Error::Database(sqlx::Error::PoolTimedOut).into())
            }
            AdvanceFailureBoundary::Publication => Ok(()),
        }
    }

    async fn before_child_publication(&self, _pending: &PendingExecutionRequested) -> Result<()> {
        Err(attune_common::Error::PermissionDenied("synthetic postcommit rejection".into()).into())
    }
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and the workflow-child-inputs RabbitMQ vhost"]
async fn managed_delegated_advancement_distinguishes_rejection_rollback_and_publication_recovery() {
    let broker = CacheBrokerFixture::connect("workflow-child-inputs").await;
    for boundary in [
        AdvanceFailureBoundary::InputValidation,
        AdvanceFailureBoundary::Database,
        AdvanceFailureBoundary::Publication,
    ] {
        let fixture = WorkflowDelegationFixture::create_managed().await.unwrap();
        let mut tx = fixture.database.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        let mut pending = WorkflowPublications::default();
        ExecutionScheduler::dispatch_workflow_task_with_conn(
            &mut tx,
            &AtomicUsize::new(0),
            &fixture.parent,
            &fixture.workflow_execution_id,
            fixture.graph.get_task("first").unwrap(),
            &fixture.context,
            None,
            None,
            &mut pending,
            &mut Vec::new(),
        )
        .await
        .unwrap();
        let completed = ExecutionRepository::update(
            &mut *tx,
            pending[0].execution_id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                result: Some(serde_json::json!({"succeeded":true})),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let transport: Arc<dyn attune_common::artifact_transport::ArtifactFileTransport> =
            Arc::new(attune_common::artifact_transport::VolumeTransport::new(
                fixture.release_files.path().to_str().unwrap(),
            ));
        let counter = AtomicUsize::new(0);
        let caches = SchedulerMetadataCaches::new();
        let outcome = ExecutionScheduler::advance_workflow_owned(
            fixture.database.pool(),
            &broker.publisher,
            &counter,
            fixture.release_files.path().to_str().unwrap(),
            &transport,
            1024 * 1024,
            100,
            None,
            &completed,
            &caches,
            Some(&AdvanceFailureObserver(boundary)),
        )
        .await;
        let parent = ExecutionRepository::find_by_id(fixture.database.pool(), fixture.parent.id)
            .await
            .unwrap()
            .unwrap();
        let children =
            ExecutionRepository::find_by_parent(fixture.database.pool(), fixture.parent.id)
                .await
                .unwrap();
        let successors: Vec<_> = children
            .iter()
            .filter(|row| {
                row.workflow_task
                    .as_ref()
                    .is_some_and(|task| task.task_name == "second")
            })
            .collect();
        match boundary {
            AdvanceFailureBoundary::InputValidation => {
                outcome.unwrap();
                assert_eq!(parent.status, ExecutionStatus::Failed);
                assert_eq!(
                    parent.result.as_ref().unwrap()["code"],
                    advancement_failure::REJECTION_CODE
                );
                assert!(!parent
                    .result
                    .unwrap()
                    .to_string()
                    .contains("synthetic private"));
                assert!(successors.is_empty());
                let workflow = WorkflowExecutionRepository::find_by_id(
                    fixture.database.pool(),
                    fixture.workflow_execution_id,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(workflow.status, ExecutionStatus::Failed);
                assert!(workflow.current_tasks.is_empty());
            }
            AdvanceFailureBoundary::Database => {
                assert!(outcome.is_err());
                assert_eq!(parent.status, ExecutionStatus::Running);
                assert!(successors.is_empty());
            }
            AdvanceFailureBoundary::Publication => {
                assert!(outcome.is_err());
                assert_eq!(parent.status, ExecutionStatus::Running);
                assert_eq!(successors.len(), 1, "committed work remains recoverable");
                assert_eq!(successors[0].status, ExecutionStatus::Requested);
                let retained_id = successors[0].id;
                ExecutionScheduler::advance_workflow_owned(
                    fixture.database.pool(),
                    &broker.publisher,
                    &counter,
                    fixture.release_files.path().to_str().unwrap(),
                    &transport,
                    1024 * 1024,
                    100,
                    None,
                    &completed,
                    &caches,
                    None,
                )
                .await
                .unwrap();
                let delivered = broker.receive().await;
                assert_eq!(
                    delivered["payload"]["execution_id"],
                    serde_json::json!(retained_id)
                );
                let after =
                    ExecutionRepository::find_by_parent(fixture.database.pool(), fixture.parent.id)
                        .await
                        .unwrap();
                assert_eq!(
                    after.len(),
                    children.len(),
                    "recovery reuses the committed child"
                );
            }
        }
        broker.assert_empty().await;
        fixture.cleanup().await.unwrap();
    }
    broker.close().await;
}
