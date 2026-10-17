//! Genuine result views feed the normal canonical evaluator and exact authored
//! output destination. No generated declaration or arbitrary derived JSON API.
use super::super::native_input_snapshot::{
    NativeInputSnapshotRepository as Snapshots, PreparedNativeResultView,
};
use super::*;

pub struct PreparedNativeValueDestination {
    destination: PreparedReceiptDestination,
    transaction: String,
    snapshot: Id,
    target: Id,
    target_version: Value,
    target_binding: String,
}
impl PreparedNativeValueDestination {
    pub fn ensure_fresh(&self) -> Result<()> {
        self.destination.fence.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.destination.fence.ensure_fresh_on(conn).await
    }
}

pub enum NativeResultViewBinding {
    Task,
    CurrentAndTask,
}
impl InheritedInputRepository {
    pub async fn prepare_native_result_view(
        conn: &mut PgConnection,
        renderer: Id,
        child: Id,
        key: &str,
    ) -> Result<PreparedNativeResultView> {
        Snapshots::prepare_native_result_view(conn, renderer, child, key).await
    }
    pub async fn replay_native_result_view(
        conn: &mut PgConnection,
        renderer: Id,
        snapshot: Id,
        key: &str,
    ) -> Result<PreparedNativeResultView> {
        Snapshots::replay_native_result_view(conn, renderer, snapshot, key).await
    }
    pub fn bind_native_result_view(
        session: &mut RenderSession,
        view: &PreparedNativeResultView,
        mode: NativeResultViewBinding,
    ) -> Result<()> {
        if session.transaction != view.transaction || session.permit.source.id != view.renderer {
            return Err(denied());
        }
        view.ensure_fresh()?;
        session.fence = session.fence.intersect(&view.fence);
        // A transformed VALUE can be staged before the source view is released,
        // without a shorthand or Condition to carry its private read for it.
        // Bind only this real session's admitted declaration. Archived views
        // issue no private capability and still require their recorded release.
        let reads = view.prospective_result_reads(&session.permit.declaration.source_path)?;
        merge_result_reads(&mut session.result_reads, &reads)?;
        let mut paths = vec![
            format!("/task/{}", escape(&view.task_name)),
            format!("/tasks/{}", escape(&view.task_name)),
        ];
        if matches!(mode, NativeResultViewBinding::CurrentAndTask) {
            paths.push("/result".into());
        }
        for path in paths {
            insert(
                session,
                path,
                view.value.clone(),
                SelectionSource::NativeSnapshot {
                    snapshot: view.snapshot,
                },
            )?;
        }
        Ok(())
    }
    /// Only an actual authored output leaf may select this view. The destination
    /// pointer is checked against its ORIGINAL declaration, not an alias map.
    pub async fn prepare_native_value_destination(
        conn: &mut PgConnection,
        view: &PreparedNativeResultView,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        key: &str,
    ) -> Result<PreparedNativeValueDestination> {
        view.ensure_fresh_on(conn).await?;
        if rendered.purpose != InputSitePurpose::Value
            || rendered.renderer.id != view.renderer
            || rendered.transaction != view.transaction
            || site.execution != rendered.renderer.owner_id
            || site.kind != InputEntityKind::ExecutionResult
            || site.purpose != InputSitePurpose::Value
            || rendered.literal.is_some()
        {
            return Err(denied());
        }
        let (body,digest):(Value,String)=sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2").bind(rendered.declaration.ok_or_else(denied)?).bind(rendered.renderer.policy_revision).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        if hash(&body)? != digest {
            return Err(denied());
        }
        let declaration: super::super::declared_input::DeclaredInputBinding =
            serde_json::from_value(body)?;
        DeclaredInputRepository::validate_declaration(conn, &rendered.renderer, &declaration)
            .await?;
        let output = declaration
            .source_path
            .strip_prefix("/workflow/output_map")
            .filter(|path| path.starts_with('/'))
            .ok_or_else(denied)?;
        if output!=site.path||!rendered.selections.iter().any(|selection|matches!(selection.source,SelectionSource::NativeSnapshot {snapshot} if snapshot==view.snapshot)) {return Err(denied());}
        let destination = Self::prepare_destination(conn, rendered, site, key).await?;
        let target_version = target_version(conn, site.execution).await?;
        let target_binding = target_binding(conn, site.execution).await?;
        Ok(PreparedNativeValueDestination {
            destination,
            transaction: view.transaction.clone(),
            snapshot: view.snapshot,
            target: site.execution,
            target_version,
            target_binding,
        })
    }
    pub async fn record_native_value(
        conn: &mut PgConnection,
        view: &PreparedNativeResultView,
        rendered: &RenderedReceiptContext,
        prepared: &PreparedNativeValueDestination,
        actual: &Value,
        key: &str,
    ) -> Result<ReceiptContext> {
        if prepared.transaction != view.transaction || prepared.snapshot != view.snapshot {
            return Err(denied());
        }
        view.require_current(conn).await?;
        if target_version(conn, prepared.target).await? != prepared.target_version
            || target_binding(conn, prepared.target).await? != prepared.target_binding
        {
            return Err(denied());
        }
        // finalize() must already have compared the actual source buffers. The
        // normal record gate also checks every original staged parent/control.
        Self::record_prepared(
            conn,
            rendered,
            &prepared.destination,
            actual,
            rendered.selections(),
            key,
        )
        .await
    }
}
async fn target_version(conn: &mut PgConnection, execution: Id) -> Result<Value> {
    sqlx::query_scalar("SELECT jsonb_build_object('result',result,'status',status,'updated',updated) FROM execution WHERE id=$1").bind(execution).fetch_optional(conn).await?.ok_or_else(denied)
}
pub(super) async fn target_binding(conn: &mut PgConnection, execution: Id) -> Result<String> {
    let binding = super::super::schema_contract::SchemaContractRepository::find(
        conn,
        super::super::schema_contract::BindingOwner::Execution,
        execution,
    )
    .await?
    .ok_or_else(denied)?;
    hash(&serde_json::to_value(binding)?)
}
