//! Generated wait failure VALUE from J's genuine same-transaction source. No
//! caller payload, ordinary child witness or public pending-read exception.
use super::super::{
    native_input_snapshot::{
        NativeInputSnapshotRepository as Snapshots, PreparedWaitCompletionOutput,
    },
    native_wait_materialization::PreparedNativeWaitCompletion,
};
use super::*;
use std::sync::Arc;

pub struct PreparedNativeWaitCompletionDestination {
    source: PreparedWaitCompletionOutput,
    parents: Vec<ReceiptContext>,
}
impl PreparedNativeWaitCompletionDestination {
    pub fn value(&self) -> &Value {
        &self.source.value
    }
    pub fn error_message(&self) -> &str {
        self.source
            .value
            .get("error")
            .and_then(Value::as_str)
            .expect("canonical wait failure")
    }
    /// A cancelled/timeout prerequisite is an unhandled failed WORKFLOW task,
    /// not cancellation of the parent workflow. Preserve that distinction.
    pub fn terminal_status(&self) -> crate::models::ExecutionStatus {
        crate::models::ExecutionStatus::Failed
    }
    pub async fn require_original_rows(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.require_original_rows(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.source.state.ensure_fresh()?;
        self.source.fence.ensure_fresh()?;
        for parent in &self.parents {
            parent.ensure_fresh()?;
        }
        Ok(())
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.state.ensure_fresh_on(conn).await?;
        require_transaction(conn, &self.source.source.transaction).await?;
        self.source.fence.ensure_fresh_on(conn).await?;
        for parent in &self.parents {
            parent.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        self.ensure_fresh()?;
        Ok(self
            .source
            .state
            .held_deadline()?
            .into_iter()
            .chain(self.source.fence.approval_deadline())
            .min())
    }
    pub fn secret_path_sources(
        &self,
        receipt: &ReceiptContext,
    ) -> Result<Vec<crate::secret_values::SecretPathSource>> {
        let snapshot = self.source.recorded.get().ok_or_else(denied)?;
        if receipt.site.execution != self.source.source.execution
            || receipt.site.kind != InputEntityKind::ExecutionResult
            || receipt.site.purpose != InputSitePurpose::Value
            || !receipt.site.path.is_empty()
            || receipt.value != self.source.value
            || !receipt.native_sources.iter().any(|source| {
                source.snapshot() == *snapshot && source.source_kind() == "wait_completion_metadata"
            })
        {
            return Err(denied());
        }
        // Protect even schema-invalid failure fallback and generated status/error.
        // C still evaluates each independent source, never a contributor union.
        Ok(vec![crate::secret_values::SecretPathSource {
            path: String::new(),
            source: crate::secret_values::SecretSource::Bound(receipt.provenance()),
        }])
    }
}

impl InheritedInputRepository {
    pub async fn prepare_native_wait_completion(
        conn: &mut PgConnection,
        state: Arc<PreparedNativeWaitCompletion>,
        key: &str,
    ) -> Result<PreparedNativeWaitCompletionDestination> {
        let mut source =
            Snapshots::prepare_wait_completion_output(conn, state.clone(), key).await?;
        let mut parents = Vec::new();
        // Keep the independent whole original wait source even when none of the
        // true/false decisions or published values happened to select its data.
        for (id, site) in source.source.parents.clone() {
            let origin = SecretOrigin::Entity {
                entity_type: site.kind.origin_kind(site.purpose),
                entity_id: id,
                path: site.path.clone(),
            };
            parents.push(Self::receipt_context(conn, &site, &origin, key).await?);
        }
        for control in state.condition_sources() {
            if control.renderer != source.renderer.id {
                return Err(denied());
            }
            for receipt in std::iter::once(&control.receipt).chain(&control.controls) {
                if receipt.site.purpose == InputSitePurpose::Value {
                    return Err(denied());
                }
                source.retain_control(receipt.id, &receipt.site)?;
                parents.push(receipt.copy_held());
            }
        }
        for value in state.issued_values() {
            source.retain_value(value.receipt.id, &value.receipt.site)?;
            parents.push(value.receipt.copy_held());
        }
        for receipt in &parents {
            receipt.ensure_fresh_on(conn).await?;
            source.fence = source.fence.intersect(&receipt.fence);
            source.retain_parent(receipt.id, &receipt.site, &receipt.origins)?;
        }
        if parents.len() > 256 {
            return Err(denied());
        }
        source.state.require_source_current(conn).await?;
        Ok(PreparedNativeWaitCompletionDestination { source, parents })
    }
    /// After J's real registration/release and ordered publication, before result
    /// ciphertext and either terminal status. No target-phase IAM recapture.
    pub async fn record_native_wait_completion(
        conn: &mut PgConnection,
        prepared: &PreparedNativeWaitCompletionDestination,
        projected: &crate::models::WorkflowExecution,
        actual: &Value,
        key: &str,
    ) -> Result<ReceiptContext> {
        prepared.ensure_fresh_on(conn).await?;
        if actual != prepared.value() {
            return Err(denied());
        }
        prepared.source.require_target(conn, projected).await?;
        let mut source_receipts = Vec::new();
        let mut public_sources = Vec::new();
        let mut outcomes = Vec::new();
        let mut native_sources = Vec::new();
        let mut parents = BTreeSet::new();
        let mut origins = prepared.source.source.origins.clone();
        for receipt in &prepared.parents {
            Self::require_finalized_native_sources(conn, &receipt.native_sources).await?;
            receipt.require_published(conn).await?;
            collect_source_receipts(receipt, &mut source_receipts)?;
            parents.insert(receipt.id);
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
            for source in &receipt.native_sources {
                if !native_sources.contains(source) {
                    native_sources.push(source.clone());
                }
            }
        }
        let (snapshot, metadata) =
            Snapshots::publish_wait_completion_output(conn, &prepared.source, projected, key)
                .await?;
        native_sources.push(metadata);
        origins.push(SecretOrigin::Entity {
            entity_type: "wait_completion_metadata".into(),
            entity_id: snapshot,
            path: String::new(),
        });
        let renderer = prepared.source.renderer.clone();
        let transaction = prepared.source.source.transaction.clone();
        let fence = prepared.source.fence.clone();
        let rendered = RenderedReceiptContext {
            transaction: transaction.clone(),
            renderer: renderer.clone(),
            declaration: None,
            literal: None,
            native: Some(NativeValueRecipe::WaitCompletion { snapshot }),
            value: actual.clone(),
            selections: vec![ReadSelection {
                context_path: "$native/value".into(),
                source: SelectionSource::NativeSnapshot { snapshot },
            }],
            fence: fence.clone(),
            purpose: InputSitePurpose::Value,
            wait_reads: vec![],
            result_reads: vec![],
            empty_reads: vec![],
        };
        let destination = PreparedReceiptDestination {
            transaction: transaction.clone(),
            renderer: renderer.id,
            admission: renderer.id,
            render_digest: render_digest(&rendered)?,
            site: InputSite {
                execution: prepared.source.source.execution,
                kind: InputEntityKind::ExecutionResult,
                purpose: InputSitePurpose::Value,
                path: String::new(),
            },
            source_receipts,
            snapshot_parents: parents.into_iter().collect(),
            native_sources,
            origins,
            public_sources,
            outcomes,
            fence,
            wait_reads: vec![],
            result_reads: vec![],
            empty_reads: vec![],
        };
        let receipt = Self::record_prepared(
            conn,
            &rendered,
            &destination,
            actual,
            &rendered.selections,
            key,
        )
        .await?;
        prepared
            .source
            .recorded
            .set(snapshot)
            .map_err(|_| denied())?;
        prepared.ensure_fresh_on(conn).await?;
        Ok(receipt)
    }
}
