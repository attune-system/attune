//! Empty-root bindings keep the prerequisite CONTROL separate from output VALUE.
use super::*;

impl NativeInputSnapshotRepository {
    pub(crate) fn compose_success_output<'a>(
        graph: &Value,
        leaves: impl Iterator<Item = (&'a str, &'a Value)>,
        contract: Option<&crate::value_schema::Contract>,
    ) -> Result<Value> {
        success_metadata::compose(graph, leaves, contract)
    }
    pub(crate) async fn require_empty_root_binding(
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        execution: Id,
        workflow: Id,
        executable: &crate::models::ExecutionExecutableSnapshot,
        contract: &super::super::schema_contract::ActionSchemaBinding,
    ) -> Result<()> {
        let (body, _, _, _) = load_encrypted(conn, snapshot).await?;
        let Source::EmptyIteration(source) = &body.source else {
            return Err(denied());
        };
        if body.renderer != renderer
            || source.execution != execution
            || source.workflow != workflow
            || serde_json::to_value(&source.executable)? != serde_json::to_value(executable)?
            || &source.schema != contract
        {
            return Err(denied());
        }
        let graph = &executable
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        let one_task = match graph.get("tasks") {
            Some(Value::Array(tasks)) => tasks.len() == 1,
            Some(Value::Object(tasks)) => tasks.len() == 1,
            _ => false,
        };
        let task = graph
            .pointer(
                source
                    .task_path
                    .strip_prefix("/workflow")
                    .ok_or_else(denied)?,
            )
            .ok_or_else(denied)?;
        let empty = |value: Option<&Value>| {
            value.is_none_or(|value| {
                value.is_null()
                    || value.as_array().is_some_and(Vec::is_empty)
                    || value.as_object().is_some_and(serde_json::Map::is_empty)
            })
        };
        if !one_task
            || !empty(graph.get("output"))
            || [
                "next",
                "publish",
                "tasks",
                "wait_for",
                "on_success",
                "on_failure",
                "on_complete",
                "on_timeout",
            ]
            .iter()
            .any(|name| !empty(task.get(*name)))
        {
            return Err(denied());
        }
        empty_iteration::require_empty_release(conn, snapshot, source).await
    }
    pub(crate) async fn empty_root_control_evidence(
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        key: &str,
    ) -> Result<SnapshotEvidence> {
        Self::evidence(
            conn,
            snapshot,
            renderer,
            "$native/value",
            "$control",
            key,
            false,
        )
        .await
    }
}
impl InheritedInputRepository {
    pub(crate) async fn empty_root_control(
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        key: &str,
    ) -> Result<ReceiptContext> {
        let (body, _, _, _) = load(conn, snapshot, key).await?;
        let Source::EmptyIteration(source) = &body.source else {
            return Err(denied());
        };
        if body.renderer != renderer {
            return Err(denied());
        }
        empty_iteration::require_empty_release(conn, snapshot, source).await?;
        Self::empty_iteration_receipt(conn, source.control, &source.control_site, false, key).await
    }
}
