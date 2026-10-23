//! Actual ordinary failure output preparation and recording through A's recipe.
use super::*;
use crate::{
    repositories::{
        inherited_input::{PreparedInputSource, PreparedNativeRecipeDestination},
        native_input_snapshot::PreparedNativeCompletionMetadata,
    },
    secret_values::SecretPathSource,
};

pub struct PreparedNativeFailureOutput {
    metadata: PreparedNativeCompletionMetadata,
    destination: PreparedNativeRecipeDestination,
}

impl NativeFailureMaterializationRepository {
    /// Source phase only. Failure never evaluates authored output_map leaves.
    pub async fn prepare_output(
        conn: &mut PgConnection,
        state: &PreparedNativeFailure,
        controls: &[&PreparedInputSource],
        key: &str,
    ) -> Result<PreparedNativeFailureOutput> {
        state.ensure_fresh_on(conn).await?;
        let metadata =
            Inputs::prepare_native_completion_metadata(conn, state, &[], controls, key).await?;
        let destination =
            Inputs::prepare_native_completion_destination(conn, &metadata, key).await?;
        Ok(PreparedNativeFailureOutput {
            metadata,
            destination,
        })
    }
}

impl PreparedNativeFailureOutput {
    /// Required pure guard before B's first target mutation. A records its held
    /// guard too; forgetting this cannot be repaired by passing a final name list.
    pub fn require_original_rows(
        &self,
        parent: &Execution,
        workflow: &WorkflowExecution,
    ) -> Result<()> {
        self.metadata.require_original_rows(parent, workflow)
    }
    /// At actual all_done, before changing parent result/status. The supplied
    /// names are equality facts from the owning scheduler, not source authority.
    pub async fn record(
        &self,
        conn: &mut PgConnection,
        failed_tasks: &[String],
        key: &str,
    ) -> Result<(Value, Vec<SecretPathSource>)> {
        self.metadata.require_projected_failure(failed_tasks)?;
        self.metadata.finalize(conn, failed_tasks).await?;
        let value = self.metadata.value().clone();
        let receipt = Inputs::record_native_completion_metadata(
            conn,
            &self.metadata,
            &self.destination,
            failed_tasks,
            &value,
            key,
        )
        .await?;
        let sources = self.metadata.secret_path_sources(&receipt)?;
        self.ensure_fresh_on(conn).await?;
        Ok((value, sources))
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.metadata.ensure_fresh_on(conn).await?;
        self.destination.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.metadata.ensure_fresh()?;
        self.destination.ensure_fresh()
    }
}
