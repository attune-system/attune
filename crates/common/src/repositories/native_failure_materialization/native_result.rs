//! Delegate original result DATA projections to A, never to template parsing.
use super::*;
use crate::repositories::{
    inherited_input::{
        InputSite, NativeResultStageBinding, NativeResultViewBinding, PreparedInputSource,
        PreparedNativeResultCondition, PreparedNativeResultStage, PreparedNativeValueDestination,
        PreparedReceiptDestination, PreparedStagedValue, ReceiptContext, RenderSession,
        RenderedReceiptContext, SelectionSource,
    },
    native_input_snapshot::PreparedNativeResultView,
};

pub(in crate::repositories) struct PreparedNativeResult {
    view: PreparedNativeResultView,
    released: std::sync::OnceLock<()>,
    view_only: bool,
    // Routing facts from actual issued receipts, not proof/evaluation memoization.
    // Never held across awaits; A independently verifies every pending ancestry.
    pending_receipts: std::sync::Mutex<BTreeSet<Id>>,
}
pub(in crate::repositories) struct PreparedResultShorthand {
    stage: PreparedNativeResultStage,
}
pub(in crate::repositories) struct PreparedResultCondition {
    condition: PreparedNativeResultCondition,
}
pub(in crate::repositories) enum ResultOutputDestination {
    Ordinary(PreparedReceiptDestination),
    Native(PreparedNativeValueDestination),
}
impl PreparedNativeResult {
    pub(in crate::repositories) fn approval_deadline(
        &self,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        self.view.fence.approval_deadline()
    }
    pub fn required(parent: &Execution, task: &str) -> Result<bool> {
        let graph = &parent
            .executable_snapshot
            .as_ref()
            .ok_or_else(changed)?
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?
            .definition;
        if graph
            .get("output_map")
            .and_then(Value::as_object)
            .is_some_and(|fields| !fields.is_empty())
        {
            return Ok(true);
        }
        for node in graph
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(changed)?
            .iter()
            .filter(|node| node.get("name").and_then(Value::as_str) == Some(task))
        {
            if CanonicalCompletionTask::prepare(node)?.is_some_and(|task| {
                task.next()
                    .iter()
                    .filter_map(|edge| edge.get("publish").and_then(Value::as_array))
                    .flatten()
                    .any(Value::is_string)
            }) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub async fn prepare(
        conn: &mut PgConnection,
        parent: Id,
        child: &Execution,
        view_only: bool,
        key: &str,
    ) -> Result<Self> {
        let view = Inputs::prepare_native_result_view(conn, parent, child.id, key).await?;
        // The model is equality input only. A loads and locks the real source;
        // compare after its factory so canonical structural gates cannot use a
        // stale delivered status with a newer, different result-view witness.
        let actual = ExecutionRepository::find_by_id(&mut *conn, child.id)
            .await?
            .ok_or_else(changed)?;
        if serde_json::to_value(&actual)? != serde_json::to_value(child)? {
            return Err(changed());
        }
        bounded(view.value())?;
        Ok(Self {
            view,
            view_only,
            released: std::sync::OnceLock::new(),
            pending_receipts: std::sync::Mutex::new(BTreeSet::new()),
        })
    }
    pub fn execution(&self) -> Id {
        self.view.source_execution()
    }
    pub fn value(&self) -> &Value {
        self.view.value()
    }
    pub fn view_only(&self) -> bool {
        self.view_only
    }
    pub async fn require_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.view.require_current(conn).await
    }
    pub fn bind(&self, session: &mut RenderSession) -> Result<()> {
        Inputs::bind_native_result_view(
            session,
            &self.view,
            NativeResultViewBinding::CurrentAndTask,
        )
    }
    pub async fn finalize(&self, conn: &mut PgConnection) -> Result<()> {
        self.view.require_current(conn).await?;
        if self.released.get().is_none() {
            self.view.finalize(conn).await?;
            self.released.set(()).map_err(|_| changed())?;
        }
        Ok(())
    }
    pub async fn prepare_shorthand(
        &self,
        conn: &mut PgConnection,
        path: &str,
        controls: &[&PreparedInputSource],
        key: &str,
    ) -> Result<PreparedResultShorthand> {
        let destination =
            Inputs::prepare_native_result_shorthand(conn, &self.view, path, controls, key).await?;
        let stage =
            Inputs::prepare_native_result_stage(conn, &self.view, &destination, key).await?;
        self.register_receipt(stage.receipt())?;
        Ok(PreparedResultShorthand { stage })
    }
    fn register_receipt(&self, receipt: &ReceiptContext) -> Result<()> {
        self.register_origin(&receipt.origin())
    }
    fn register_origin(&self, origin: &crate::secret_provenance::SecretOrigin) -> Result<()> {
        let crate::secret_provenance::SecretOrigin::Entity { entity_id, .. } = origin else {
            return Err(changed());
        };
        let mut pending = self.pending_receipts.lock().map_err(|_| changed())?;
        if !pending.contains(entity_id) && pending.len() >= MAX_CHILDREN {
            return Err(changed());
        }
        pending.insert(*entity_id);
        Ok(())
    }
    pub fn depends_on_view(&self, rendered: &RenderedReceiptContext) -> Result<bool> {
        let pending = self.pending_receipts.lock().map_err(|_| changed())?;
        Ok(rendered
            .selections()
            .iter()
            .any(|selection| match &selection.source {
                SelectionSource::NativeSnapshot { snapshot } => *snapshot == self.view.snapshot(),
                SelectionSource::Inherited { receipt, .. }
                | SelectionSource::Control { receipt, .. } => pending.contains(receipt),
                _ => false,
            }))
    }
    pub fn retain_staged_lineage(
        &self,
        rendered: &RenderedReceiptContext,
        staged: &PreparedStagedValue,
    ) -> Result<()> {
        if self.depends_on_view(rendered)? {
            if rendered.value() != staged.value() {
                return Err(changed());
            }
            for origin in &staged.provenance().origins {
                self.register_origin(origin)?;
            }
        }
        Ok(())
    }
    pub async fn prepare_condition(
        &self,
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        key: &str,
    ) -> Result<PreparedResultCondition> {
        let condition =
            Inputs::prepare_native_result_condition(conn, &self.view, rendered, site, key).await?;
        self.register_receipt(condition.receipt())?;
        Ok(PreparedResultCondition { condition })
    }
    pub async fn prepare_output_destination(
        &self,
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        key: &str,
    ) -> Result<ResultOutputDestination> {
        if rendered.selections().iter().any(|selection| matches!(selection.source, SelectionSource::NativeSnapshot { snapshot } if snapshot == self.view.snapshot())) {
            Ok(ResultOutputDestination::Native(Inputs::prepare_native_value_destination(conn, &self.view, rendered, site, key).await?))
        } else {
            Ok(ResultOutputDestination::Ordinary(Inputs::prepare_destination(conn, rendered, site, key).await?))
        }
    }
    pub async fn record_output(
        &self,
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        destination: &ResultOutputDestination,
        actual: &Value,
        key: &str,
    ) -> Result<ReceiptContext> {
        self.finalize(conn).await?;
        match destination {
            ResultOutputDestination::Ordinary(destination) => {
                Inputs::record_prepared(
                    conn,
                    rendered,
                    destination,
                    actual,
                    rendered.selections(),
                    key,
                )
                .await
            }
            ResultOutputDestination::Native(destination) => {
                Inputs::record_native_value(conn, &self.view, rendered, destination, actual, key)
                    .await
            }
        }
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.view.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.view.ensure_fresh()
    }
}
impl PreparedResultShorthand {
    pub(in crate::repositories) fn approval_deadline(
        &self,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        self.stage.receipt().approval_deadline()
    }
    pub fn value(&self) -> &Value {
        self.stage.receipt().accepted_value()
    }
    pub fn require_original_workflow(&self, actual: &WorkflowExecution) -> Result<()> {
        self.stage.require_original_workflow(actual)
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.stage.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.stage.ensure_fresh()
    }
    pub fn receipt(&self) -> &ReceiptContext {
        self.stage.receipt()
    }
    pub fn bind(&self, session: &mut RenderSession, mode: NativeResultStageBinding) -> Result<()> {
        Inputs::bind_native_result_stage(session, &self.stage, mode)
    }
    pub async fn finalize(&self, conn: &mut PgConnection, key: &str) -> Result<()> {
        Inputs::finalize_native_result_stage(conn, &self.stage, key).await
    }
}
impl PreparedResultCondition {
    pub fn source(&self) -> &PreparedInputSource {
        self.condition.source()
    }
    pub async fn finalize(&self, conn: &mut PgConnection) -> Result<()> {
        Inputs::finalize_native_result_condition(conn, &self.condition).await
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.condition.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.condition.ensure_fresh()
    }
}
impl ResultOutputDestination {
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Self::Native(destination) = self {
            destination.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if let Self::Native(destination) = self {
            destination.ensure_fresh()?;
        }
        Ok(())
    }
}
