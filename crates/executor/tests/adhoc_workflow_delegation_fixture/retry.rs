async fn ad_hoc_retry_fixture() -> (AdhocWorkflowDelegationFixture, Execution) {
    use attune_common::repositories::workflow::UpdateWorkflowExecutionInput;
    let fixture = AdhocWorkflowDelegationFixture::create_with_selection(true).await.unwrap();
    let pool = fixture.database.pool();
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
        fixture.graph.get_task("retry_single").unwrap(),
        &fixture.context,
        None,
        None,
        &mut messages,
        &mut Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1);
    WorkflowExecutionRepository::update(
        &mut *tx,
        fixture.workflow_execution_id,
        UpdateWorkflowExecutionInput {
            current_tasks: Some(vec!["retry_single".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let child = ExecutionRepository::find_by_parent(pool, fixture.parent.id)
        .await
        .unwrap()
        .remove(0);
    let source = ExecutionRepository::update(
        pool,
        child.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Failed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (fixture, source)
}

fn ad_hoc_retry_metadata(source: &Execution) -> attune_common::models::WorkflowTaskMetadata {
    let mut task = source.workflow_task.clone().unwrap();
    task.retry_count += 1;
    task.next_retry_at = Some(Utc::now());
    task.started_at = None;
    task.completed_at = None;
    task.duration_ms = None;
    task.timed_out = false;
    task
}

#[tokio::test]
async fn ad_hoc_workflow_retry_preserves_inputs_lineage_and_recovers_the_same_attempt() {
    use attune_common::repositories::{
        native_retry_materialization::NativeRetryMaterializationRepository as Retries,
        requested_execution_publication::RequestedExecutionPublicationRepository as Publication,
    };
    let (fixture, source) = ad_hoc_retry_fixture().await;
    let pool = fixture.database.pool();
    let workflow = WorkflowExecutionRepository::find_by_id(pool, fixture.workflow_execution_id)
        .await
        .unwrap()
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let prepared = Retries::prepare(&mut tx, &source, &workflow, "fixture-key")
        .await
        .unwrap();
    let retry = prepared
        .materialize(&mut tx, ad_hoc_retry_metadata(&source))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(retry.config, source.config);
    assert_eq!(retry.executor, source.executor);
    assert_eq!(retry.permission_set_refs, source.permission_set_refs);
    assert_eq!(retry.parent, source.parent);
    assert_eq!(retry.original_execution, Some(source.id));
    assert_eq!(retry.retry_count, 1);
    assert_eq!(
        serde_json::to_value(&retry.executable_snapshot).unwrap(),
        serde_json::to_value(&source.executable_snapshot).unwrap()
    );
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_authority_admission WHERE owner_kind='execution' AND owner_id=ANY($1)")
        .bind(vec![source.id, retry.id]).fetch_one(pool).await.unwrap();
    assert_eq!(
        records, 0,
        "ad hoc execution must not borrow pack authority"
    );
    let mut duplicate = pool.begin().await.unwrap();
    let prepared = Retries::prepare(&mut duplicate, &source, &workflow, "fixture-key")
        .await
        .unwrap();
    let same = prepared
        .materialize(&mut duplicate, ad_hoc_retry_metadata(&source))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(same.id, retry.id);
    duplicate.commit().await.unwrap();
    let mut recovery = pool.begin().await.unwrap();
    let publication = Publication::prepare(&mut recovery, &retry, "fixture-key")
        .await
        .unwrap()
        .unwrap();
    publication.lock_target(&mut recovery).await.unwrap();
    publication.ensure_fresh_on(&mut recovery).await.unwrap();
    assert_eq!(publication.execution().id, retry.id);
    recovery.commit().await.unwrap();
    assert_eq!(
        ExecutionRepository::find_by_parent(pool, fixture.parent.id)
            .await
            .unwrap()
            .len(),
        2
    );
    fixture.cleanup().await.unwrap();
}

#[tokio::test]
async fn ad_hoc_workflow_retry_rechecks_live_owner_and_rejects_changed_inputs() {
    use attune_common::repositories::{
        identity::{IdentityRepository, UpdateIdentityInput},
        native_retry_materialization::NativeRetryMaterializationRepository as Retries,
    };
    let (fixture, source) = ad_hoc_retry_fixture().await;
    let pool = fixture.database.pool();
    let workflow = WorkflowExecutionRepository::find_by_id(pool, fixture.workflow_execution_id)
        .await
        .unwrap()
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let prepared = Retries::prepare(&mut tx, &source, &workflow, "fixture-key")
        .await
        .unwrap();
    let retry = prepared
        .materialize(&mut tx, ad_hoc_retry_metadata(&source))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut changed = retry.clone();
    changed.config = Some(serde_json::json!({"substituted":true}));
    let mut publication = pool.begin().await.unwrap();
    assert!(
        Retries::prepare_publication(&mut publication, &changed, source.id, "fixture-key")
            .await
            .is_err()
    );
    publication.rollback().await.unwrap();
    IdentityRepository::update(
        pool,
        fixture.identity_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut revoked = pool.begin().await.unwrap();
    assert!(
        Retries::prepare(&mut revoked, &source, &workflow, "fixture-key")
            .await
            .is_err()
    );
    revoked.rollback().await.unwrap();
    fixture.cleanup().await.unwrap();
}
