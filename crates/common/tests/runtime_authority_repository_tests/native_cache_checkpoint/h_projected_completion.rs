//! Reuse B's canonical admitted-cache fixture and actual checkpoint writer.
//! Model changes below are equality-negative inputs, never source witnesses.
use super::*;
use attune_common::repositories::{
    native_cache_materialization::SourceCacheCompletion,
    native_failure_materialization::{
        NativeFailureMaterializationRepository as Failure, ProjectedCacheCompletionState,
    },
};

pub(super) async fn check_source(
    conn: &mut sqlx::PgConnection,
    source: &SourceCacheCompletion,
) -> ProjectedCacheCompletionState {
    let root = source.execution().id;
    let workflow = source.workflow().id;
    assert!(
        Failure::prepare_projected_cache_completion(conn, -1, workflow, source)
            .await
            .is_err()
    );
    assert!(
        Failure::prepare_projected_cache_completion(conn, root, -1, source)
            .await
            .is_err()
    );
    let plan = Failure::prepare_projected_cache_completion(conn, root, workflow, source)
        .await
        .unwrap()
        .expect("single-cache root has an authenticated finite projection");
    assert!(std::ptr::eq(plan.source(), source));
    plan.require_current(conn).await.unwrap();
    plan.ensure_fresh_on(conn).await.unwrap();
    plan.ensure_fresh().unwrap();
    let state = plan.projection();
    assert_eq!(state.execution(), root);
    assert_eq!(state.workflow(), workflow);
    assert_eq!(state.task_name(), "cached");
    assert_eq!(state.succeeded(), !source.has_failed_latest_attempt());
    if source.has_failed_latest_attempt() {
        assert_eq!(state.failed_tasks(), &["cached".to_owned()]);
        assert!(state.completed_tasks().is_empty());
        assert_eq!(
            state.metadata(),
            &json!({"succeeded":false,"error":"Workflow failed: 1 task(s) failed: cached"})
        );
    } else {
        assert!(state.failed_tasks().is_empty());
        assert_eq!(state.completed_tasks(), &["cached".to_owned()]);
        assert_eq!(state.metadata(), &json!({"succeeded":true}));
    }
    state.require_metadata(state.metadata()).unwrap();
    assert!(state
        .require_metadata(&json!({"succeeded":"{{ false }}"}))
        .is_err());
    let mut equality_only = source.workflow().clone();
    equality_only.current_tasks.clear();
    equality_only.failed_tasks = state.failed_tasks().to_vec();
    equality_only.completed_tasks = state.completed_tasks().to_vec();
    equality_only.status = if state.succeeded() {
        ExecutionStatus::Completed
    } else {
        ExecutionStatus::Failed
    };
    state.require_projected_task_state(&equality_only).unwrap();
    equality_only.failed_tasks.push("unbacked".into());
    assert!(state.require_projected_task_state(&equality_only).is_err());
    // End the borrow before B's actual &mut checkpoint apply. This pure state
    // does not stand in for B's source fences or A's future receipt capability.
    plan.into_projection()
}

#[tokio::test]
async fn h_projected_cache_root_uses_all_real_terminal_batches_and_actual_checkpoint_state() {
    super::super::run_native_cache_fixture(Some(Case::CompletionSource)).await;
}
#[tokio::test]
async fn h_projected_cache_root_failure_is_backed_by_the_actual_failed_latest_attempt() {
    super::super::run_native_cache_fixture(Some(Case::CompletionFailed)).await;
}
#[tokio::test]
async fn h_projected_cache_root_rejects_changed_original_attempt_before_progression() {
    super::super::run_native_cache_fixture(Some(Case::CompletionChanged)).await;
}
#[tokio::test]
async fn h_projected_cache_root_initial_empty_eof_has_no_child_witness() {
    super::super::run_native_cache_fixture(Some(Case::Empty)).await;
}
#[tokio::test]
async fn h_projected_cache_root_persisted_empty_eof_has_no_child_witness() {
    super::super::run_native_cache_fixture(Some(Case::CompletionPersistedEmpty)).await;
}
