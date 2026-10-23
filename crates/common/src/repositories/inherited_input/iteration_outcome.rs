//! Whole-group lifecycle binding from D's genuine nonempty completion source.
use super::super::native_input_snapshot::{
    NativeInputSnapshotRepository as Snapshots, PreparedNativeIterationOutcome,
};
use super::*;

impl InheritedInputRepository {
    pub(crate) async fn original_iteration_cardinality(
        conn: &mut PgConnection,
        renderer: &RuntimeAdmissionRecord,
        control: &ReceiptContext,
    ) -> Result<usize> {
        require_transaction(conn, &control.transaction).await?;
        if control.site.execution != renderer.owner_id
            || control.site.kind != InputEntityKind::WorkflowVariables
            || control.site.purpose != InputSitePurpose::Control
        {
            return Err(denied());
        }
        let (_, batch) = iteration_shape(conn, renderer, &control.site.path).await?;
        let count = control.value.as_array().map_or(1, Vec::len);
        Ok(count.div_ceil(batch.unwrap_or(1)))
    }
    pub async fn prepare_native_iteration_outcome(
        conn: &mut PgConnection,
        source: &std::sync::Arc<
            super::super::native_iteration_materialization::PreparedNativeIterationCompletion,
        >,
        key: &str,
    ) -> Result<PreparedNativeIterationOutcome> {
        Snapshots::prepare_native_iteration_outcome(conn, source, key).await
    }
    /// Only canonical status functions in this actual task's authored next/when.
    /// No per-child status or result VALUE is substituted for the group.
    pub fn bind_native_iteration_outcome(
        session: &mut RenderSession,
        source: &PreparedNativeIterationOutcome,
    ) -> Result<()> {
        if session.transaction != source.transaction
            || session.permit.source.id != source.renderer
            || session.purpose != InputSitePurpose::Condition
        {
            return Err(denied());
        }
        let suffix = session
            .permit
            .declaration
            .source_path
            .strip_prefix(&format!("{}/next/", source.task_path))
            .ok_or_else(denied)?;
        let tokens = suffix.split('/').collect::<Vec<_>>();
        if tokens.len() != 2 || tokens[0].parse::<usize>().is_err() || tokens[1] != "when" {
            return Err(denied());
        }
        source.ensure_fresh()?;
        session.fence = session.fence.intersect(&source.fence);
        for name in ["succeeded", "failed", "timed_out", "cancelled"] {
            let value = match name {
                "succeeded" => source.status == "completed",
                "failed" => source.status == "failed",
                _ => false,
            };
            insert(
                session,
                format!("/$outcome/{name}"),
                Value::Bool(value),
                SelectionSource::NativeIterationOutcome {
                    snapshot: source.snapshot,
                },
            )?;
        }
        Ok(())
    }
    /// An independent group CONTROL dependency for an already canonical literal
    /// or expression. It does not choose a branch or alter the rendered VALUE.
    pub fn attach_native_iteration_control(
        rendered: &mut RenderedReceiptContext,
        source: &PreparedNativeIterationOutcome,
    ) -> Result<()> {
        if rendered.transaction != source.transaction || rendered.renderer.id != source.renderer {
            return Err(denied());
        }
        source.ensure_fresh()?;
        rendered.fence = rendered.fence.intersect(&source.fence);
        let selected = ReadSelection {
            context_path: "$control".into(),
            source: SelectionSource::NativeIterationOutcome {
                snapshot: source.snapshot,
            },
        };
        if !rendered.selections.contains(&selected) {
            rendered.selections.push(selected);
        }
        if rendered.selections.len() > 256 {
            return Err(denied());
        }
        Ok(())
    }
}
