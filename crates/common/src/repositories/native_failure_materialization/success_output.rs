//! Ordinary all-done output from A's actual generated-success recipe. The owning
//! B plan and H source state stay alive; no payload/diagnostic DTO supplies proof.
use super::*;
use crate::{
    repositories::{
        inherited_input::{PreparedNativeSuccessDestination, ReceiptContext},
        native_transition_materialization::PreparedNativeTransitions,
    },
    secret_values::SecretPathSource,
};
use std::sync::OnceLock;

pub struct PreparedNativeSuccessOutput<'a> {
    state: PreparedNativeSuccess<'a>,
    destination: PreparedNativeSuccessDestination,
    original_checked: OnceLock<()>,
    receipt: OnceLock<ReceiptContext>,
}

impl NativeFailureMaterializationRepository {
    /// Source phase only. None preserves the separate owners of fanout, retry,
    /// successors, cache and wait; it is never an authorization fallback.
    pub async fn prepare_success_output<'a>(
        conn: &mut PgConnection,
        root: Id,
        workflow: Id,
        completed: Id,
        plan: &'a PreparedNativeTransitions,
        key: &str,
    ) -> Result<Option<PreparedNativeSuccessOutput<'a>>> {
        let Some(state) =
            Self::prepare_native_success(conn, root, workflow, completed, plan).await?
        else {
            return Ok(None);
        };
        let destination = Inputs::prepare_native_success_metadata(conn, &state, key).await?;
        let prepared = PreparedNativeSuccessOutput {
            state,
            destination,
            original_checked: OnceLock::new(),
            receipt: OnceLock::new(),
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(Some(prepared))
    }
}

impl PreparedNativeSuccessOutput<'_> {
    pub fn value(&self) -> &Value {
        self.destination.value()
    }
    pub fn require_completed_execution(&self, actual: &Execution) -> Result<()> {
        let original = self
            .state
            .source_executions()
            .find(|child| child.id == self.state.completed_execution())
            .ok_or_else(changed)?;
        if actual.id != self.state.completed_execution()
            || serde_json::to_value(actual)? != serde_json::to_value(original)?
        {
            return Err(changed());
        }
        Ok(())
    }
    /// Both complete original rows are checked on EVERY call. A's mandatory
    /// one-shot marker is issued only once, before the first target/log write.
    pub fn require_original_rows(
        &self,
        parent: &Execution,
        workflow: &WorkflowExecution,
    ) -> Result<()> {
        self.ensure_fresh()?;
        self.state.require_original_rows(parent, workflow)?;
        if self.original_checked.get().is_none() {
            self.destination.require_original_rows(parent, workflow)?;
            self.original_checked.set(()).map_err(|_| changed())?;
        }
        Ok(())
    }
    /// Actual persisted Running task state after B's one canonical record call.
    /// A rechecks the genuine frame, whole original inventory and target rows.
    pub async fn record(
        &self,
        conn: &mut PgConnection,
        projected: &WorkflowExecution,
        key: &str,
    ) -> Result<(Value, Vec<SecretPathSource>)> {
        if self.original_checked.get().is_none()
            || self.receipt.get().is_some()
            || projected.status != ExecutionStatus::Running
        {
            return Err(changed());
        }
        self.state.require_projected_workflow(projected)?;
        self.ensure_fresh_on(conn).await?;
        let value = self.destination.value().clone();
        let receipt = Inputs::record_native_success_metadata(
            conn,
            &self.state,
            &self.destination,
            projected,
            &value,
            key,
        )
        .await?;
        let sources = self.destination.secret_path_sources(&receipt)?;
        self.receipt.set(receipt).map_err(|_| changed())?;
        self.ensure_fresh_on(conn).await?;
        Ok((value, sources))
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.state.ensure_fresh_on(conn).await?;
        self.destination.ensure_fresh_on(conn).await?;
        let _ = self.destination.held_deadline()?;
        if let Some(receipt) = self.receipt.get() {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.state.ensure_fresh()?;
        self.destination.ensure_fresh()?;
        let _ = self.destination.held_deadline()?;
        if let Some(receipt) = self.receipt.get() {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
}
