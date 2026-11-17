//! Failure of an uncommitted advance is distinct from publication recovery.
use super::*;

pub(super) const REJECTION_CODE: &str = "workflow_input_admission_rejected";
const REJECTION_MESSAGE: &str =
    "Workflow advancement rejected invalid execution authority or inputs";

pub(super) enum RejectionOutcome {
    PreserveRecorded,
    Handled(Option<PendingExecutionCompleted>),
}

/// Only deterministic authorization/contract failures can terminate a graph.
/// Transport, database, cleanup and unknown failures keep their recovery path.
pub(super) fn is_rejection(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<attune_common::Error>(),
            Some(
                attune_common::Error::PermissionDenied(_)
                    | attune_common::Error::Validation(_)
                    | attune_common::Error::SchemaValidation(_)
                    | attune_common::Error::Encryption(_)
            )
        )
    })
}

impl ExecutionScheduler {
    /// The caller owns a fresh SQLx transaction AFTER acknowledged rollback and
    /// holds the workflow advisory lock. Never call after any commit attempt.
    pub(super) fn reject_workflow_advance<'a>(
        conn: &'a mut PgConnection,
        completed: &'a Execution,
        encryption_key: Option<&'a str>,
    ) -> futures::future::BoxFuture<'a, Result<RejectionOutcome>> {
        Box::pin(async move {
            let task = completed
                .workflow_task
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Rejected advance has no workflow lineage"))?;
            let workflow = WorkflowExecutionRepository::find_by_id_for_update(
                &mut *conn,
                task.workflow_execution,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("Rejected workflow is unavailable"))?;
            let child = ExecutionRepository::find_by_id(&mut *conn, completed.id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Completed workflow child is unavailable"))?;
            anyhow::ensure!(
                child.parent == Some(workflow.execution)
                    && completed.parent == child.parent
                    && child.status == completed.status
                    && matches!(
                        child.status,
                        ExecutionStatus::Completed
                            | ExecutionStatus::Failed
                            | ExecutionStatus::Cancelled
                            | ExecutionStatus::Timeout
                            | ExecutionStatus::Abandoned
                    )
                    && child
                        .workflow_task
                        .as_ref()
                        .is_some_and(|actual| { actual.workflow_execution == workflow.id }),
                "Rejected advance differs from its persisted workflow lineage"
            );
            let parent = ExecutionRepository::find_by_id_for_update(&mut *conn, workflow.execution)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Rejected workflow parent is unavailable"))?;
            // Classification does not authorize execution or source reads.
            // Recorded continuations retain their source/receipt recovery contract.
            if attune_common::repositories::execution_admission::ExecutionAdmissionRepository::selected_authority(conn, parent.id).await?
                == attune_common::repositories::execution_runtime_admission::ExistingExecutionAuthority::Recorded
            {
                return Ok(RejectionOutcome::PreserveRecorded);
            }
            if matches!(
                parent.status,
                ExecutionStatus::Completed
                    | ExecutionStatus::Failed
                    | ExecutionStatus::Cancelled
                    | ExecutionStatus::Timeout
                    | ExecutionStatus::Abandoned
            ) {
                return Ok(RejectionOutcome::Handled(
                    Self::workflow_completion_publication(conn, &parent).await?,
                ));
            }
            if matches!(parent.status, ExecutionStatus::Canceling)
                || matches!(
                    workflow.status,
                    ExecutionStatus::Canceling | ExecutionStatus::Cancelled
                )
            {
                return Ok(RejectionOutcome::Handled(None));
            }
            // Diagnostics never include the denied values, raw error, or secret
            // provenance. Existing output-channel policy still protects `error`.
            InquiryRepository::cancel_pending_for_workflow(&mut *conn, workflow.id).await?;
            WorkflowTaskWaitRepository::cancel_waiting_for_workflow(
                &mut *conn,
                workflow.id,
                serde_json::json!({"reason": REJECTION_CODE}),
            )
            .await?;
            let failed = Self::complete_workflow_with_conn(
                conn,
                parent.id,
                workflow.id,
                false,
                Some(REJECTION_MESSAGE),
                Some(serde_json::json!({
                    "code": REJECTION_CODE,
                    "error": REJECTION_MESSAGE,
                })),
                encryption_key,
            )
            .await?;
            attune_common::repositories::workflow_log_outbox::WorkflowLogOutboxRepository::enqueue_seal(
                conn, workflow.id,
            ).await?;
            Ok(RejectionOutcome::Handled(
                Self::workflow_completion_publication(conn, &failed).await?,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_classification_excludes_transport_and_database_failures() {
        assert!(is_rejection(
            &attune_common::Error::PermissionDenied("fixture".into()).into()
        ));
        assert!(is_rejection(
            &attune_common::Error::Validation("fixture".into()).into()
        ));
        assert!(!is_rejection(
            &attune_common::Error::Database(sqlx::Error::PoolTimedOut).into()
        ));
        assert!(!is_rejection(&MqError::Publish("fixture".into()).into()));
        assert!(!is_rejection(&anyhow::anyhow!("unknown failure")));
    }
}
