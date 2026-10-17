//! Deferred ordinary-success publication uses only source-phase held contexts.
//! The generated root VALUE is never disguised as an authored leaf or Condition.
use super::super::{
    native_failure_materialization::PreparedNativeSuccess,
    native_input_snapshot::{
        NativeInputSnapshotRepository as Snapshots, PreparedNativeSuccessMetadata,
    },
};
use super::*;

fn success_publication_stage(error: Error, stage: &'static str) -> Error {
    match error {
        Error::PermissionDenied(message) => Error::PermissionDenied(format!(
            "Ordinary success publication [{stage}] denied: {message}"
        )),
        other => other,
    }
}

pub struct PreparedNativeSuccessDestination {
    source: PreparedNativeSuccessMetadata,
    leaves: Vec<(String, PreparedReceiptDestination)>,
}
impl PreparedNativeSuccessDestination {
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        self.source.ensure_fresh()?;
        let mut deadline = self.source.fence().approval_deadline();
        for (_, destination) in &self.leaves {
            deadline = deadline
                .into_iter()
                .chain(destination.held_deadline()?)
                .min();
        }
        Ok(deadline)
    }
    pub fn secret_path_sources(
        &self,
        receipt: &ReceiptContext,
    ) -> Result<Vec<crate::secret_values::SecretPathSource>> {
        self.source.secret_path_sources(receipt)
    }
    pub fn value(&self) -> &Value {
        self.source.value()
    }
    /// Mandatory before the caller's first parent/workflow write.
    pub fn require_original_rows(
        &self,
        parent: &crate::models::Execution,
        workflow: &crate::models::WorkflowExecution,
    ) -> Result<()> {
        self.source.require_original_rows(parent, workflow)
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.source.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.ensure_fresh_on(conn).await
    }
}
impl InheritedInputRepository {
    pub async fn prepare_native_success_metadata(
        conn: &mut PgConnection,
        state: &PreparedNativeSuccess<'_>,
        key: &str,
    ) -> Result<PreparedNativeSuccessDestination> {
        let mut source = Snapshots::prepare_success_observation(conn, state, key).await?;
        let mut leaves = Vec::new();
        for (path, rendered) in state.output_plan().prepared_output_leaves() {
            let site = InputSite {
                execution: state.execution(),
                kind: InputEntityKind::ExecutionResult,
                purpose: InputSitePurpose::Value,
                path: path.to_owned(),
            };
            let destination = Self::prepare_destination(conn, rendered, &site, key).await?;
            source.retain_destination(&destination)?;
            leaves.push((path.to_owned(), destination));
        }
        state.ensure_fresh_on(conn).await?;
        Ok(PreparedNativeSuccessDestination { source, leaves })
    }
    /// After B's real frame publication and actual task-list persistence, before
    /// result ciphertext or either terminal status. This calls the canonical
    /// receipt writer with B's original opaque leaf renders, without reevaluation.
    pub async fn record_native_success_metadata(
        conn: &mut PgConnection,
        state: &PreparedNativeSuccess<'_>,
        prepared: &PreparedNativeSuccessDestination,
        projected: &crate::models::WorkflowExecution,
        actual: &Value,
        key: &str,
    ) -> Result<ReceiptContext> {
        prepared.source.require_original_guard()?;
        state.require_projected_workflow(projected)?;
        if actual != prepared.value() || projected.status != crate::models::ExecutionStatus::Running
        {
            return Err(denied());
        }
        Snapshots::require_success_target(conn, &prepared.source, state, projected)
            .await
            .map_err(|error| success_publication_stage(error, "target"))?;
        let expected = state
            .output_plan()
            .prepared_output_leaves()
            .collect::<Vec<_>>();
        if expected.len() != prepared.leaves.len() {
            return Err(denied());
        }
        let mut leaves = Vec::new();
        for ((path, rendered), (held_path, destination)) in
            expected.into_iter().zip(&prepared.leaves)
        {
            if path != held_path {
                return Err(denied());
            }
            let value = actual.pointer(path).ok_or_else(denied)?;
            let receipt = Self::record_prepared(
                conn,
                rendered,
                destination,
                value,
                rendered.selections(),
                key,
            )
            .await?;
            leaves.push((path.to_owned(), receipt));
        }
        let (snapshot, metadata) = Snapshots::publish_success_observation(
            conn,
            &prepared.source,
            state,
            projected,
            &leaves,
            actual,
            key,
        )
        .await
        .map_err(|error| success_publication_stage(error, "snapshot"))?;
        let renderer = prepared.source.renderer().clone();
        let transaction = super::transaction(conn).await?;
        let fence = prepared.source.fence().clone();
        let rendered = RenderedReceiptContext {
            transaction: transaction.clone(),
            renderer: renderer.clone(),
            declaration: None,
            literal: None,
            native: Some(NativeValueRecipe::OrdinarySuccess { snapshot }),
            value: actual.clone(),
            selections: vec![ReadSelection {
                context_path: "$native/value".into(),
                source: SelectionSource::NativeSnapshot { snapshot },
            }],
            fence: fence.clone(),
            purpose: InputSitePurpose::Value,
            wait_reads: Vec::new(),
            result_reads: Vec::new(),
        };
        // No prepare_destination call under target locks. Preserve every held
        // receipt descriptor and its independent ceiling instead of recapturing
        // authority from a current identity or current publisher schema.
        let mut source_receipts = Vec::new();
        let mut origins = Vec::new();
        let mut public_sources = Vec::new();
        let mut outcomes = Vec::new();
        let mut native_sources = vec![metadata];
        let mut parents = BTreeSet::new();
        for receipt in prepared
            .source
            .parents()
            .iter()
            .chain(leaves.iter().map(|(_, receipt)| receipt))
        {
            receipt.ensure_fresh_on(conn).await?;
            collect_source_receipts(receipt, &mut source_receipts)?;
            parents.insert(receipt.id);
            for origin in &receipt.origins {
                if !origins.contains(origin) {
                    origins.push(origin.clone());
                }
            }
            for site in &receipt.public_sources {
                if !public_sources.contains(site) {
                    public_sources.push(site.clone());
                }
            }
            for outcome in &receipt.outcomes {
                if !outcomes.contains(outcome) {
                    outcomes.push(outcome.clone());
                }
            }
            for native in &receipt.native_sources {
                if !native_sources.contains(native) {
                    native_sources.push(native.clone());
                }
            }
        }
        origins.push(SecretOrigin::Entity {
            entity_type: "ordinary_success_metadata".into(),
            entity_id: snapshot,
            path: String::new(),
        });
        let destination = PreparedReceiptDestination {
            transaction,
            renderer: renderer.id,
            admission: renderer.id,
            render_digest: render_digest(&rendered)?,
            site: InputSite {
                execution: state.execution(),
                kind: InputEntityKind::ExecutionResult,
                purpose: InputSitePurpose::Value,
                path: String::new(),
            },
            result_reads: Vec::new(),
            wait_reads: Vec::new(),
            source_receipts,
            snapshot_parents: parents.into_iter().collect(),
            native_sources,
            origins,
            public_sources,
            outcomes,
            fence,
        };
        let receipt = Self::record_prepared(
            conn,
            &rendered,
            &destination,
            actual,
            &rendered.selections,
            key,
        )
        .await
        .map_err(|error| success_publication_stage(error, "receipt"))?;
        prepared.source.mark_recorded(snapshot)?;
        state.ensure_fresh_on(conn).await?;
        Ok(receipt)
    }
}
