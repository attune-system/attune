//! Source-phase authored output evaluation for an actual terminal empty task.
use super::*;

pub struct PreparedEmptyRootReplay {
    parent: Execution,
    receipts: Vec<ReceiptContext>,
}
impl PreparedEmptyRootReplay {
    pub fn completed_parent(&self) -> &Execution {
        &self.parent
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
}
impl NativeIterationMaterializationRepository {
    /// Read-only duplicate recovery from a terminal root's finite receipt. This
    /// never reevaluates with_items, output leaves or a transition condition.
    pub async fn prepare_empty_root_replay(
        conn: &mut PgConnection,
        execution: Id,
        key: &str,
    ) -> Result<PreparedEmptyRootReplay> {
        let parent = Executions::find_by_id(&mut *conn, execution)
            .await?
            .ok_or_else(changed)?;
        let workflow = Workflows::find_by_execution(&mut *conn, execution)
            .await?
            .ok_or_else(changed)?;
        let receipts = Inputs::replay_empty_root(conn, &parent, &workflow, key).await?;
        Ok(PreparedEmptyRootReplay { parent, receipts })
    }
}

impl PreparedNativeIteration {
    pub(super) fn is_terminal_empty_root(&self) -> bool {
        if self.total != 0
            || self.empty_outcome.is_none()
            || self.context.parent.result.is_some()
            || !self.context.workflow.failed_tasks.is_empty()
        {
            return false;
        }
        let Some(graph) = self
            .context
            .parent
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
            .map(|definition| &definition.definition)
        else {
            return false;
        };
        let one_task = match graph.get("tasks") {
            Some(Value::Array(tasks)) => tasks.len() == 1,
            Some(Value::Object(tasks)) => tasks.len() == 1,
            _ => false,
        };
        let empty = |value: Option<&Value>| {
            value.is_none_or(|value| {
                value.is_null()
                    || value.as_array().is_some_and(Vec::is_empty)
                    || value.as_object().is_some_and(serde_json::Map::is_empty)
            })
        };
        one_task
            && [
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
            .all(|name| empty(self.context.task.get(*name)))
            && empty(graph.get("output"))
    }

    pub(super) async fn prepare_empty_root(
        &self,
        conn: &mut PgConnection,
        key: &str,
    ) -> Result<super::super::inherited_input::PreparedEmptyRoot> {
        if !self.is_terminal_empty_root() || self.context.frame.is_some() {
            return Err(changed());
        }
        self.empty_source()?.require_source_current(conn).await?;
        let graph = &self
            .context
            .parent
            .executable_snapshot
            .as_ref()
            .ok_or_else(changed)?
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?
            .definition;
        let mut leaves = Vec::new();
        if let Some(authored) = graph.get("output_map").filter(|value| !value.is_null()) {
            if !authored.is_object() {
                return Err(changed());
            }
            let mut authored_leaves = Vec::new();
            collect_leaves(authored, String::new(), &mut authored_leaves, 0)?;
            for (path, leaf) in authored_leaves {
                // The empty object is structure, not an authored output leaf.
                if path.is_empty() {
                    continue;
                }
                let source_path = format!("/workflow/output_map{path}");
                let rendered = if let Some(binding) = self.context.bindings.get(&source_path) {
                    let mut session = Inputs::begin_render(
                        conn,
                        self.context.parent.id,
                        &binding.declaration_revision,
                        &source_path,
                        &binding.expression,
                        InputSitePurpose::Value,
                    )
                    .await?;
                    let names = expression_names(&binding.expression)?;
                    for field in &self.context.sources.fields {
                        for context_path in &field.paths {
                            let namespace = context_path.split('/').nth(1).unwrap_or_default();
                            if !names.contains(namespace) {
                                continue;
                            }
                            if let Some(source) = &field.source {
                                Inputs::bind_prepared(
                                    &mut session,
                                    context_path,
                                    source,
                                    source.value(),
                                )?;
                            } else {
                                Inputs::bind_public_field(
                                    conn,
                                    &mut session,
                                    context_path,
                                    &field.site,
                                )
                                .await?;
                            }
                        }
                    }
                    if names.contains("system") {
                        Inputs::bind_prepared_system(&mut session, &self.context.sources.system)?;
                    }
                    if binding.uses_pack_configuration() {
                        Inputs::bind_configuration(conn, &mut session, key).await?;
                    }
                    Inputs::evaluate_with_keys(conn, session, key).await?
                } else {
                    if contains_template(&leaf) {
                        return Err(changed());
                    }
                    Inputs::literal(
                        conn,
                        self.context.parent.id,
                        &self.context.revision,
                        &source_path,
                    )
                    .await?
                };
                leaves.push((path, rendered));
            }
        }
        Inputs::prepare_empty_root(
            conn,
            &self.context.parent,
            &self.context.workflow,
            self.empty_outcome.as_ref().ok_or_else(changed)?,
            leaves,
            key,
        )
        .await
    }
}
