//! Prepare original source proofs before workflow target locks and mutations.
use super::*;
use attune_common::repositories::{
    native_failure_materialization::{
        NativeFailureMaterializationRepository as Failure, PreparedNativeFailure,
        PreparedNativeFailureOutput, PreparedNativeSuccessOutput,
    },
    native_input_origin::{NativeInputClosure, NativeInputOriginRepository},
    native_transition_materialization::{
        NativeTransitionMaterializationRepository as Transitions, PreparedNativeTransitions,
    },
};
use std::sync::OnceLock;

pub(super) struct WorkflowAdvanceSources<'plan> {
    pub failure: Option<PreparedNativeFailure>,
    pub failure_output: Option<PreparedNativeFailureOutput>,
    pub success_output: Option<Box<PreparedNativeSuccessOutput<'plan>>>,
    pub sources: Option<NativeInputClosure>,
    pub context: Option<WorkflowContext>,
}

impl ExecutionScheduler {
    /// The stable transition owner outlives both preparation and publication.
    /// Heap-own this phase separately: its native poll temporaries must not
    /// overlap the later task-dispatch poll frames on a completion thread.
    pub(super) fn prepare_workflow_advance_sources<'a, 'plan: 'a>(
        conn: &'a mut PgConnection,
        execution: &'a Execution,
        task: &'a WorkflowTaskMetadata,
        encryption_key: Option<&'a str>,
        transition_owner: &'plan OnceLock<Option<PreparedNativeTransitions>>,
    ) -> futures::future::BoxFuture<'a, Result<WorkflowAdvanceSources<'plan>>> {
        Box::pin(async move {
            let failure = if execution.id > 0 && task.task_index.is_none() && task.retry_count == 0
            {
                Failure::prepare_for_completion(conn, execution.id).await?
            } else {
                None
            };
            if let Some(plan) = &failure {
                plan.require_completed_execution(execution)?;
            }
            let transitions = if let Some(key) = encryption_key {
                Transitions::prepare_for_completion(conn, execution, key).await?
            } else {
                None
            };
            transition_owner
                .set(transitions)
                .map_err(|_| anyhow::anyhow!("Native transition owner already initialized"))?;
            let transitions = transition_owner.get().and_then(Option::as_ref);
            let success_output =
                if execution.id > 0 && execution.status == ExecutionStatus::Completed {
                    if let (Some(plan), Some(key)) = (transitions, encryption_key) {
                        Failure::prepare_success_output(
                            conn,
                            plan.execution(),
                            plan.workflow().id,
                            execution.id,
                            plan,
                            key,
                        )
                        .await?
                        .map(Box::new)
                    } else {
                        None
                    }
                } else {
                    None
                };
            let sources = if let Some(plan) = transitions {
                Some(
                    NativeInputOriginRepository::capture_workflow_with_key(
                        conn,
                        plan.execution(),
                        encryption_key,
                    )
                    .await?,
                )
            } else if let Some(state) = &failure {
                Some(
                    NativeInputOriginRepository::capture_workflow_with_key(
                        conn,
                        state.execution(),
                        encryption_key,
                    )
                    .await?,
                )
            } else {
                None
            };
            let failure_output = if let Some(state) = &failure {
                if state.requires_transition_plan()? && transitions.is_none() {
                    return Err(anyhow::anyhow!(
                        "Native failure completion requires its prepared transition decisions"
                    ));
                }
                let controls = transitions
                    .map(|plan| plan.completion_decisions())
                    .unwrap_or_default();
                Some(
                    Failure::prepare_output(
                        conn,
                        state,
                        &controls,
                        encryption_key.ok_or_else(|| {
                            anyhow::anyhow!("Native failure output encryption is unavailable")
                        })?,
                    )
                    .await?,
                )
            } else {
                None
            };
            let context = if let (Some(plan), Some(sources)) = (transitions, &sources) {
                Some(
                    Self::prepare_native_transition_context(conn, plan, sources, encryption_key)
                        .await?,
                )
            } else {
                None
            };
            Ok(WorkflowAdvanceSources {
                failure,
                failure_output,
                success_output,
                sources,
                context,
            })
        })
    }
}
