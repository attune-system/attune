//! Constructive empty-list evidence. Missing dispatches never construct this
//! capability; only the original canonical list render can issue it.
use super::*;

pub struct NativeEmptyIterationSource<'a> {
    source: &'a PreparedNativeIteration,
}

impl PreparedNativeIteration {
    pub fn needs_empty_activation(&self) -> bool {
        self.total == 0
            && self
                .context
                .task
                .get("next")
                .and_then(Value::as_array)
                .is_some_and(|next| !next.is_empty())
    }
    pub(crate) fn empty_source(&self) -> Result<NativeEmptyIterationSource<'_>> {
        let control = self.iteration_control.as_ref().ok_or_else(changed)?;
        let receipt = self.empty_control.as_ref().ok_or_else(changed)?;
        let array = self.array.as_ref().ok_or_else(changed)?;
        if self.total != 0
            || array.value() != &json!([])
            || control.value() != array.value()
            || receipt.accepted_value() != control.value()
            || receipt.origin() != control.origin()
            || control.site().path != format!("{}/with_items", self.context.path)
            || control.site().execution != self.context.parent.id
            || !self.children.is_empty()
            || !self.original_children.is_empty()
            || !self.observations.is_empty()
            || self.active_attempts != 0
        {
            return Err(changed());
        }
        Ok(NativeEmptyIterationSource { source: self })
    }

    pub fn empty_outcome(
        &self,
    ) -> Option<&super::super::native_input_snapshot::PreparedNativeEmptyIterationOutcome> {
        self.empty_outcome.as_ref()
    }
}

impl NativeEmptyIterationSource<'_> {
    pub fn parent(&self) -> &Execution {
        &self.source.context.parent
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
    pub fn control(&self) -> &PreparedIterationControl {
        // Construction requires this exact held control, never a caller receipt.
        self.source
            .iteration_control
            .as_ref()
            .expect("issued empty source")
    }
    pub fn receipts(&self) -> impl Iterator<Item = &ReceiptContext> {
        self.source.controls.iter().chain(&self.source.receipts)
    }
    pub fn control_receipt(&self) -> &ReceiptContext {
        self.source
            .empty_control
            .as_ref()
            .expect("issued empty Control receipt")
    }
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let source = self.source;
        let mut dates = vec![
            source.parent_fence.approval_deadline(),
            source.context.sources.held_deadline()?,
            self.control_receipt().approval_deadline(),
        ];
        dates.extend(self.receipts().map(ReceiptContext::approval_deadline));
        dates.extend(
            source
                .recovery_fences
                .iter()
                .map(ApprovalEffectFence::approval_deadline),
        );
        dates.extend(
            source
                .context
                .outcome
                .iter()
                .map(PreparedWorkflowOutcome::approval_deadline),
        );
        dates.push(source.context.frame_deadline);
        Ok(dates.into_iter().flatten().min())
    }
    pub async fn require_source_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.empty_source()?;
        self.source.ensure_fresh_on(conn).await?;
        if serde_json::to_value(
            Executions::find_by_id(&mut *conn, self.parent().id)
                .await?
                .ok_or_else(changed)?,
        )? != serde_json::to_value(self.parent())?
            || serde_json::to_value(
                Workflows::find_by_id(&mut *conn, self.workflow().id)
                    .await?
                    .ok_or_else(changed)?,
            )? != serde_json::to_value(self.workflow())?
            || Secrets::find_stored_by_entity(
                &mut *conn,
                ENTITY_WORKFLOW_VARIABLES,
                self.parent().id,
            )
            .await?
                != self.source.original_secrets
        {
            return Err(changed());
        }
        self.require_zero_dispatches(conn).await
    }
    pub async fn require_zero_dispatches(&self, conn: &mut PgConnection) -> Result<()> {
        if original_dispatch_count(conn, self.workflow().id, self.task_name()).await? != 0
            || !initial_children(conn, self.parent().id, self.workflow().id, self.task_name())
                .await?
                .is_empty()
            || !latest_observations(conn, self.parent().id, self.workflow().id, self.task_name())
                .await?
                .is_empty()
        {
            return Err(changed());
        }
        self.control().ensure_fresh_on(conn).await
    }
    pub(crate) async fn require_projected(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(frame) = &self.source.context.frame {
            frame.require_published(conn).await?;
            let current = Workflows::find_by_id(&mut *conn, self.workflow().id)
                .await?
                .ok_or_else(changed)?;
            let mut original = self.workflow().clone();
            // Only B's private publication seal authorizes these changed fields.
            original.variables = current.variables.clone();
            original.updated = current.updated;
            if serde_json::to_value(original)? != serde_json::to_value(current)? {
                return Err(changed());
            }
        } else {
            self.require_source_current(conn).await?;
        }
        super::super::accepted_execution_source::same_execution(
            self.parent(),
            &Executions::find_by_id(&mut *conn, self.parent().id)
                .await?
                .ok_or_else(changed)?,
        )?;
        self.require_zero_dispatches(conn).await
    }
}
