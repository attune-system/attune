//! Authenticated whole-iteration status source. This token never represents a
//! synthetic execution, one selected child, or an aggregate result VALUE.
use super::*;

pub struct PreparedNativeIterationCompletion {
    source: PreparedNativeIteration,
    status: ExecutionStatus,
}

/// Read-only member of a factory-issued whole-iteration source. Membership and
/// outcome pairing are selected by D, never supplied by a proof consumer.
pub struct NativeIterationCompletionMember<'a> {
    observation: &'a IterationObservation,
    outcome: &'a PreparedWorkflowOutcome,
}

impl<'a> NativeIterationCompletionMember<'a> {
    pub fn execution(&self) -> Id {
        self.observation.execution
    }
    pub fn original_execution(&self) -> Id {
        self.observation.original
    }
    pub fn ordinal(&self) -> i32 {
        self.observation.ordinal
    }
    pub fn task_batch(&self) -> Option<i32> {
        self.observation.batch
    }
    pub fn retry_count(&self) -> i32 {
        self.observation.attempt
    }
    pub fn metadata_retry_count(&self) -> i32 {
        self.observation.metadata_attempt
    }
    pub fn created(&self) -> &'a chrono::DateTime<chrono::Utc> {
        &self.observation.created
    }
    pub fn status(&self) -> ExecutionStatus {
        self.observation.status
    }
    pub fn outcome(&self) -> &'a PreparedWorkflowOutcome {
        self.outcome
    }
}

impl NativeIterationMaterializationRepository {
    /// Source phase for a retained, nonempty ordinary sibling set. An empty
    /// activation needs a capability from its original retained list evaluation,
    /// not a factory that interprets missing dispatch rows as an empty list.
    pub async fn prepare_completion(
        conn: &mut PgConnection,
        parent: Id,
        task: &str,
        key: &str,
    ) -> Result<Option<PreparedNativeIterationCompletion>> {
        let parent = Executions::find_by_id(&mut *conn, parent)
            .await?
            .ok_or_else(changed)?;
        if parent.status != ExecutionStatus::Running || !Self::is_ordinary_task(&parent, task) {
            return Ok(None);
        }
        let workflow = Workflows::find_by_execution(&mut *conn, parent.id)
            .await?
            .ok_or_else(changed)?;
        if workflow.status != ExecutionStatus::Running {
            return Ok(None);
        }
        let count = original_dispatch_count(conn, workflow.id, task).await?;
        if count == 0 {
            return Ok(None);
        }
        let observed = latest_observations(conn, parent.id, workflow.id, task).await?;
        if observed.len() != count {
            return Err(changed());
        }
        if observed.iter().any(|item| {
            !matches!(
                item.status,
                ExecutionStatus::Completed
                    | ExecutionStatus::Failed
                    | ExecutionStatus::Timeout
                    | ExecutionStatus::Cancelled
                    | ExecutionStatus::Abandoned
            )
        }) {
            return Ok(None);
        }
        let source = Self::prepare(conn, &parent, task, None, key).await?;
        if source.original_children.len() != count
            || source.observations != observed
            || !source.children.is_empty()
            || source.observed_outcomes.len() != count
        {
            return Err(changed());
        }
        let status = if observed
            .iter()
            .all(|item| item.status == ExecutionStatus::Completed)
        {
            ExecutionStatus::Completed
        } else {
            ExecutionStatus::Failed
        };
        let token = PreparedNativeIterationCompletion { source, status };
        token.require_source_current(conn).await?;
        Ok(Some(token))
    }
}

impl PreparedNativeIterationCompletion {
    /// Read-only export of every independently held source deadline. This date
    /// is not a credential or replacement for transaction/member/freshness checks.
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let source = &self.source;
        // This factory issues retained nonempty completions, not new/pending
        // materialization plans. Fail closed if that contract is extended without
        // extending the deadline fold for the newly held capability types too.
        if self.ordinal_count() == 0
            || source.original_children.len() != self.ordinal_count()
            || source.observed_outcomes.len() != self.ordinal_count()
            || !source.children.is_empty()
            || source.array.is_some()
            || source.iteration_control.is_some()
            || source.context.frame.is_some()
            || source.empty_control.is_some()
            || source.empty_outcome.is_some()
        {
            return Err(changed());
        }
        let expected_inputs: BTreeSet<_> = source
            .original_children
            .iter()
            .map(|child| child.id)
            .chain(source.observations.iter().map(|member| member.execution))
            .collect();
        let held_inputs: BTreeSet<_> = source
            .observed_inputs
            .iter()
            .map(|input| input.source.id)
            .collect();
        if expected_inputs != held_inputs || source.observed_inputs.len() != expected_inputs.len() {
            return Err(changed());
        }
        let mut deadlines = vec![
            Ok(source.parent_fence.approval_deadline()),
            source.context.sources.held_deadline(),
        ];
        if let Some(outcome) = &source.context.outcome {
            deadlines.push(Ok(outcome.approval_deadline()));
        }
        deadlines.extend(
            source
                .controls
                .iter()
                .chain(&source.receipts)
                .map(|receipt| Ok(receipt.approval_deadline())),
        );
        // Includes canonical acceptance/admission fences for ORIGINAL and latest
        // retry inputs, not merely the outcome for a delivered or selected item.
        deadlines.extend(
            source
                .recovery_fences
                .iter()
                .map(|fence| Ok(fence.approval_deadline())),
        );
        deadlines.extend(
            source
                .observed_outcomes
                .iter()
                .chain(&source.original_outcomes)
                .map(|outcome| Ok(outcome.approval_deadline())),
        );
        complete_held_deadline(deadlines)
    }
    pub fn original_parent(&self) -> &Execution {
        &self.source.context.parent
    }
    pub fn original_siblings(&self) -> &[Execution] {
        &self.source.original_children
    }
    pub fn input_receipts(&self) -> &[ReceiptContext] {
        &self.source.receipts
    }
    pub fn controls(&self) -> &[ReceiptContext] {
        &self.source.controls
    }
    pub fn members(
        &self,
    ) -> impl ExactSizeIterator<Item = NativeIterationCompletionMember<'_>> + '_ {
        self.source
            .observations
            .iter()
            .zip(&self.source.observed_outcomes)
            .map(|(observation, outcome)| NativeIterationCompletionMember {
                observation,
                outcome,
            })
    }
    pub fn renderer(&self) -> Id {
        self.source.context.parent.id
    }
    pub fn workflow(&self) -> &WorkflowExecution {
        &self.source.context.workflow
    }
    pub fn task_name(&self) -> &str {
        &self.source.context.task_name
    }
    pub fn task_path(&self) -> &str {
        &self.source.context.path
    }
    /// A lifecycle fact from complete latest-ordinal coverage, not a VALUE
    /// capability. Canonical expressions must use an official aggregate binder.
    pub fn status(&self) -> ExecutionStatus {
        self.status
    }
    pub fn ordinal_count(&self) -> usize {
        self.source.observations.len()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.source.ensure_source_fresh()
    }
    pub async fn require_source_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        let source = &self.source;
        if serde_json::to_value(
            Executions::find_by_id(&mut *conn, self.renderer())
                .await?
                .ok_or_else(changed)?,
        )? != serde_json::to_value(&source.context.parent)?
            || serde_json::to_value(
                Workflows::find_by_id(&mut *conn, self.workflow().id)
                    .await?
                    .ok_or_else(changed)?,
            )? != serde_json::to_value(self.workflow())?
            || Secrets::find_stored_by_entity(
                &mut *conn,
                ENTITY_WORKFLOW_VARIABLES,
                self.renderer(),
            )
            .await?
                != source.original_secrets
        {
            return Err(changed());
        }
        self.require_members_current(conn).await
    }
    /// Post-publication comparison. Only original membership and held fences are
    /// checked here. Legitimate B variable publications do not replace the
    /// original full pre-write comparison or require a caller-provided baseline.
    pub async fn require_members_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        if original_dispatch_count(conn, self.workflow().id, self.task_name()).await?
            != self.ordinal_count()
            || latest_observations(conn, self.renderer(), self.workflow().id, self.task_name())
                .await?
                != self.source.observations
            || serde_json::to_value(
                initial_children(conn, self.renderer(), self.workflow().id, self.task_name())
                    .await?,
            )? != serde_json::to_value(&self.source.original_children)?
        {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await
    }
}

fn complete_held_deadline(
    deadlines: impl IntoIterator<Item = Result<Option<chrono::DateTime<chrono::Utc>>>>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    deadlines.into_iter().try_fold(None, |earliest, next| {
        Ok(earliest.into_iter().chain(next?).min())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_deadline_includes_independent_false_control_and_acceptance_fences() {
        let late = chrono::DateTime::from_timestamp(4_000_000_000, 0).unwrap();
        let early = chrono::DateTime::from_timestamp(3_900_000_000, 0).unwrap();
        let parent = ApprovalEffectFence::from_input_approval(None).unwrap();
        let acceptance = ApprovalEffectFence::from_input_approval(Some(late)).unwrap();
        // No Boolean/child Value is constructed or consulted by the fold. A
        // retained false decision's independent fence is just as binding.
        let false_control = ApprovalEffectFence::from_input_approval(Some(early)).unwrap();
        assert_eq!(
            complete_held_deadline([
                Ok(parent.approval_deadline()),
                Ok(acceptance.approval_deadline()),
                Ok(false_control.approval_deadline())
            ])
            .unwrap(),
            Some(early)
        );
    }

    #[test]
    fn held_deadline_none_requires_all_components_known_unbounded() {
        assert_eq!(complete_held_deadline([Ok(None), Ok(None)]).unwrap(), None);
        assert!(complete_held_deadline([Ok(None), Err(changed())]).is_err());
        let known = chrono::DateTime::from_timestamp(4_000_000_000, 0).unwrap();
        assert!(complete_held_deadline([Ok(Some(known)), Err(changed())]).is_err());
    }
}
