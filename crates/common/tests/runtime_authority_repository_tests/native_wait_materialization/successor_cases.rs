//! Real ordinary predecessor -> selected terminal Wait -> retained root output.
use super::*;
use attune_common::repositories::{
    native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
    native_wait_materialization::{
        InitialWaitApply, NativeInitialWaitMaterializationRepository as Initial,
    },
    workflow::UpdateWorkflowExecutionInput,
    Update,
};

pub(super) async fn run(
    pool: &sqlx::PgPool,
    workflow: Id,
    parent: Id,
    task: &str,
    target: Target,
    key: &str,
    mode: u8,
    expiry: chrono::DateTime<Utc>,
) {
    let Target::Execution(source_id) = target else {
        panic!("owned ordinary execution required")
    };
    let file_context = if matches!(mode, 0 | 3) {
        let identity = super::output_release_cases::file_producer(pool, parent, key).await;
        let directory = tempfile::tempdir().unwrap();
        let store =
            attune_common::blob_store::FilesystemBlobStore::new(directory.path().join("objects"))
                .unwrap();
        Some((identity, directory, store))
    } else {
        None
    };
    let mut setup = pool.begin().await.unwrap();
    protect(&mut setup).await;
    status(&mut setup, target, "timeout").await;
    Workflows::update(
        &mut *setup,
        workflow,
        UpdateWorkflowExecutionInput {
            current_tasks: Some(vec!["source".into()]),
            completed_tasks: (mode == 5).then(|| vec![task.into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    setup.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    protect(&mut tx).await;
    let source = ExecutionRepository::find_by_id(&mut *tx, source_id)
        .await
        .unwrap()
        .unwrap();
    if mode == 5 {
        let before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM runtime_input_receipt WHERE execution=$1")
                .bind(parent)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert!(Transitions::prepare_wait_successor(&mut tx, &source, key)
            .await
            .unwrap()
            .is_none());
        let after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM runtime_input_receipt WHERE execution=$1")
                .bind(parent)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(
            before, after,
            "completed target must not cause another Condition render"
        );
        assert!(Waits::find_by_workflow_task(&mut *tx, workflow, task)
            .await
            .unwrap()
            .is_none());
        tx.rollback().await.unwrap();
        return;
    }
    let predecessor = Transitions::prepare_wait_successor(&mut tx, &source, key)
        .await
        .unwrap()
        .expect("actual timed-out predecessor must select its Wait");
    assert_eq!(predecessor.target(), task);
    assert_eq!(predecessor.original().current_tasks, vec!["source"]);
    assert_eq!(predecessor.projected().current_tasks, vec![task]);
    assert_eq!(predecessor.projected().completed_tasks, vec!["source"]);
    let mut plan = Initial::prepare_successor(&mut tx, predecessor.clone(), key)
        .await
        .unwrap();
    assert!(Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .is_none());
    if mode == 1 {
        Workflows::update(
            &mut *tx,
            workflow,
            UpdateWorkflowExecutionInput {
                current_tasks: Some(vec!["unrelated".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            matches!(
                predecessor.publish(&mut tx, key).await,
                Err(attune_common::Error::PermissionDenied(_))
            ),
            "changed predecessor state must deny"
        );
        tx.rollback().await.unwrap();
        assert!(Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .is_none());
        return;
    }
    if mode == 2 {
        let mut other = pool.begin().await.unwrap();
        let denied = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            predecessor.ensure_fresh_on(&mut other),
        )
        .await
        .expect("foreign transaction rejection must not wait for targets");
        assert!(matches!(
            denied,
            Err(attune_common::Error::PermissionDenied(_))
        ));
        other.rollback().await.unwrap();
        tx.rollback().await.unwrap();
        assert!(Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .is_none());
        return;
    }
    if mode == 4 {
        let mut blocker = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let observed_pool = pool.clone();
        let release = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                let mut observed = false;
                loop {
                    let blocked: bool = sqlx::query_scalar("SELECT $2=ANY(pg_blocking_pids($1))")
                        .bind(waiter)
                        .bind(blocker_pid)
                        .fetch_one(&observed_pool)
                        .await
                        .unwrap();
                    let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>=$1")
                        .bind(expiry)
                        .fetch_one(&observed_pool)
                        .await
                        .unwrap();
                    if blocked && !observed {
                        assert!(
                            !expired,
                            "approval expired before observed workflow target wait"
                        );
                        observed = true;
                    }
                    if observed && blocked && expired {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            blocker.rollback().await.unwrap();
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(70), async {
            predecessor.publish(&mut tx, key).await?;
            plan.apply(&mut tx, key).await
        })
        .await
        .unwrap();
        release.await.unwrap();
        assert!(matches!(
            result,
            Err(attune_common::Error::PermissionDenied(_))
        ));
        tx.rollback().await.unwrap();
        assert!(Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .is_none());
        let actual = Workflows::find_by_id(pool, workflow)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.current_tasks, vec!["source"]);
        assert!(!actual.completed_tasks.contains(&"source".into()));
        let parent = ExecutionRepository::find_by_id(pool, parent)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parent.status, ExecutionStatus::Running);
        return;
    }
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(workflow)
        .execute(&mut *tx)
        .await
        .unwrap();
    predecessor.publish(&mut tx, key).await.unwrap();
    let applied = plan.apply(&mut tx, key).await.unwrap();
    assert!(matches!(applied, InitialWaitApply::Recovered(ref children) if children.is_empty()));
    assert_eq!(plan.completions().len(), 1);
    assert_eq!(plan.completions()[0].id, parent);
    assert_eq!(plan.completions()[0].status, ExecutionStatus::Failed);
    predecessor.ensure_fresh_on(&mut tx).await.unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    predecessor.ensure_fresh().unwrap();
    plan.ensure_fresh().unwrap();
    let actual = Workflows::find_by_id(pool, workflow)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, ExecutionStatus::Failed);
    assert!(actual.completed_tasks.contains(&"source".into()));
    assert_eq!(actual.failed_tasks, vec![task]);
    let children = ExecutionRepository::find_by_parent(pool, parent)
        .await
        .unwrap();
    assert!(!children.iter().any(|child| child
        .workflow_task
        .as_ref()
        .is_some_and(|meta| meta.task_name == task)));
    let Some(((credential, human, pack_ref), directory, store)) = file_context else {
        panic!("positive successor cases require physical file custody");
    };
    let wait = Waits::find_by_workflow_task(pool, workflow, task)
        .await
        .unwrap()
        .unwrap();
    let mut read = pool.begin().await.unwrap();
    protect(&mut read).await;
    let release: (Id, Id) = sqlx::query_as("SELECT snapshot,original_snapshot FROM runtime_native_wait_completion_release WHERE execution=$1 AND workflow=$2 AND wait=$3")
        .bind(parent).bind(workflow).bind(wait.id).fetch_one(&mut *read).await.unwrap();
    let evidence = attune_common::repositories::native_input_snapshot::NativeInputSnapshotRepository::storage_evidence(
        &mut read, release.0).await.unwrap();
    let task_path = evidence
        .wait_completion()
        .unwrap()
        .original_task_path()
        .to_owned();
    assert!(matches!(attune_common::repositories::native_input_snapshot::NativeInputSnapshotRepository::prepare_released_wait(
        &mut read, parent, &task_path, wait.id, key).await,
        Err(attune_common::Error::PermissionDenied(_))), "terminal replay must not reopen live delivery");
    let fields = Secrets::find_stored_by_entity(&mut *read, "execution_result", parent)
        .await
        .unwrap();
    let (ordinary, receipts) =
        attune_common::repositories::immutable_input_origin::prepare_input_receipts(
            &mut read,
            "execution_result",
            parent,
            &fields,
            key,
        )
        .await
        .unwrap();
    assert!(ordinary.is_empty());
    assert_eq!(receipts.len(), 1);
    read.commit().await.unwrap();
    let held = super::super::file_native_retention_cases::hold_wait_output(
        pool,
        &credential,
        human,
        &pack_ref,
        key,
        &receipts[0],
        directory.path(),
        &store,
    )
    .await;
    let pins: Vec<Id> = sqlx::query_scalar("SELECT source FROM file_evidence_pin WHERE owner=$1 AND kind='runtime_native_input_snapshot'")
        .bind(held.version).fetch_all(pool).await.unwrap();
    assert!(pins.contains(&release.0) && pins.contains(&release.1));
    reclaim(pool, target).await;
    assert!(ExecutionRepository::find_by_id(pool, source_id)
        .await
        .unwrap()
        .is_none());
    let mut cold = pool.begin().await.unwrap();
    protect(&mut cold).await;
    let mut replay = attune_common::repositories::native_wait_materialization::NativeWaitCompletionReplayRepository::prepare(
        &mut cold, &wait, Some(key)).await.unwrap().unwrap();
    assert!(replay.apply(&mut cold, key).await.unwrap().is_empty());
    assert_eq!(replay.completions().len(), 1);
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
    let mut sources =
        attune_common::repositories::native_input_disclosure::NativeWaitSources::default();
    sources
        .prepare_context(&mut cold, &receipts[0], key, 0)
        .await
        .unwrap();
    sources.ensure_held_on(&mut cold).await.unwrap();
    replay.ensure_fresh_on(&mut cold).await.unwrap();
    cold.commit().await.unwrap();
    replay.ensure_fresh().unwrap();
    super::output_release_cases::settle_file(
        pool,
        held,
        directory.path(),
        &store,
        [release.0, release.1],
    )
    .await;
    directory.close().unwrap();
}
