//! Prospective ordinary with_items composition. The original Control value and
//! authored batch option remain unchanged, including scalar/batch-one semantics.
use super::super::workflow::WorkflowExecutionRepository;
use super::*;

pub struct PreparedIterationControl {
    renderer: Id,
    receipt: ReceiptContext,
    batch_size: Option<usize>,
    workflow: Id,
    workflow_original: Value,
    original_checked: std::sync::OnceLock<()>,
}
impl PreparedIterationControl {
    pub(crate) fn held_receipt(&self) -> &ReceiptContext {
        &self.receipt
    }
    pub(crate) fn held_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.receipt.approval_deadline()
    }
    pub fn value(&self) -> &Value {
        &self.receipt.value
    }
    pub fn site(&self) -> &InputSite {
        &self.receipt.site
    }
    pub fn origin(&self) -> SecretOrigin {
        self.receipt.origin()
    }
    pub fn batch_size(&self) -> Option<usize> {
        self.batch_size
    }
    pub fn item_count(&self) -> usize {
        self.value().as_array().map_or(1, Vec::len)
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.receipt.ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.receipt.ensure_fresh_on(conn).await
    }
    /// Call with the actual owner-loaded locked row, before its FIRST mutation.
    pub fn require_original_workflow(
        &self,
        actual: &crate::models::WorkflowExecution,
    ) -> Result<()> {
        if serde_json::to_value(actual)? != self.workflow_original {
            return Err(denied());
        }
        self.original_checked.get_or_init(|| ());
        Ok(())
    }
    /// After real upstream VALUE publication, before child config receipts,
    /// first seal or dispatch. Only held descriptors/binding facts are consumed.
    pub async fn finalize(&self, conn: &mut PgConnection) -> Result<()> {
        if self.original_checked.get().is_none() {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await?;
        InheritedInputRepository::require_finalized_native_sources(
            conn,
            &self.receipt.native_sources,
        )
        .await?;
        require_published_sources(conn, &self.receipt.source_receipts).await?;
        let changed=sqlx::query("UPDATE runtime_native_iteration_control SET publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1 AND workflow=$2 AND prepared_transaction=pg_current_xact_id()::TEXT AND publication_transaction IS NULL").bind(self.receipt.id).bind(self.workflow).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        let changed=sqlx::query("UPDATE runtime_input_receipt_staged SET publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1 AND prepared_transaction=pg_current_xact_id()::TEXT AND publication_transaction IS NULL").bind(self.receipt.id).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await
    }
}

impl InheritedInputRepository {
    /// Source phase. No supplied list, normalized array, batch option, revision
    /// assertion or caller receipt can replace the single canonical render.
    pub async fn prepare_iteration_control(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        key: &str,
    ) -> Result<PreparedIterationControl> {
        require_transaction(conn, &rendered.transaction).await?;
        if rendered.native.is_some() || rendered.renderer.owner_kind != "execution" {
            return Err(Error::PermissionDenied(
                "Iteration control renderer shape denied".into(),
            ));
        }
        let path = match (rendered.declaration, &rendered.literal) {
            (Some(id), None) if rendered.purpose == InputSitePurpose::Control => {
                let (body,expected):(Value,String)=sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2").bind(id).bind(rendered.renderer.policy_revision).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
                if hash(&body)? != expected {
                    return Err(denied());
                }
                let declaration: super::super::declared_input::DeclaredInputBinding =
                    serde_json::from_value(body)?;
                DeclaredInputRepository::validate_declaration(
                    conn,
                    &rendered.renderer,
                    &declaration,
                )
                .await?;
                declaration.source_path
            }
            (None, Some(literal)) if rendered.purpose == InputSitePurpose::Value => {
                if literal_value(conn, &rendered.renderer, literal).await? != rendered.value {
                    return Err(Error::PermissionDenied(
                        "Iteration control original literal mismatch".into(),
                    ));
                }
                literal.path.clone()
            }
            _ => {
                return Err(Error::PermissionDenied(
                    "Iteration control render purpose denied".into(),
                ))
            }
        };
        if !with_items_path(&path) {
            return Err(denied());
        }
        let (definition, batch_size) = iteration_shape(conn, &rendered.renderer, &path).await?;
        let stored_batch = batch_size
            .map(i64::try_from)
            .transpose()
            .map_err(|_| denied())?;
        let workflow =
            WorkflowExecutionRepository::find_by_execution(&mut *conn, rendered.renderer.owner_id)
                .await?
                .ok_or_else(denied)?;
        if workflow.workflow_def != definition {
            return Err(denied());
        }
        let source = RenderedReceiptContext {
            result_reads: rendered.result_reads.clone(),
            empty_reads: rendered.empty_reads.clone(),
            wait_reads: rendered.wait_reads.clone(),
            native: None,
            transaction: rendered.transaction.clone(),
            renderer: rendered.renderer.clone(),
            declaration: rendered.declaration,
            literal: rendered.literal.clone(),
            value: rendered.value.clone(),
            selections: rendered.selections.clone(),
            fence: rendered.fence.clone(),
            purpose: InputSitePurpose::Control,
        };
        let site = InputSite {
            execution: rendered.renderer.owner_id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Control,
            path,
        };
        let destination = Self::prepare_destination(conn, &source, &site, key)
            .await
            .map_err(|error| control_phase(error, "destination"))?;
        let receipt = Self::record_prepared_inner(
            conn,
            &source,
            &destination,
            &source.value,
            &source.selections,
            key,
            true,
        )
        .await
        .map_err(|error| control_phase(error, "private staging"))?;
        sqlx::query("INSERT INTO runtime_native_iteration_control(receipt,execution,workflow,workflow_definition,task_path,batch_size) VALUES($1,$2,$3,$4,$5,$6)").bind(receipt.id).bind(site.execution).bind(workflow.id).bind(definition).bind(&site.path).bind(stored_batch).execute(&mut *conn).await?;
        sqlx::query("INSERT INTO runtime_input_receipt_staged(receipt) VALUES($1)")
            .bind(receipt.id)
            .execute(&mut *conn)
            .await?;
        // Reopen through the same bounded graph walker with every genuine
        // carried source capability, not just the Wait subset.
        let mut receipt = Self::preparation_receipt_context_with_admissions(
            conn,
            &site,
            &receipt.origin(),
            key,
            &receipt.wait_reads,
            &receipt.result_reads,
            &receipt.empty_reads,
            &mut RetainedReceiptAdmissions::default(),
        )
        .await
        .map_err(|error| control_phase(error, "canonical staging reopen"))?;
        receipt.newly_recorded = true;
        Ok(PreparedIterationControl {
            renderer: rendered.renderer.id,
            receipt,
            batch_size,
            workflow: workflow.id,
            workflow_original: serde_json::to_value(workflow)?,
            original_checked: std::sync::OnceLock::new(),
        })
    }
    pub fn prepare_iteration_item_from_control(
        control: &PreparedIterationControl,
        ordinal: usize,
    ) -> Result<PreparedIterationItem> {
        control.ensure_fresh()?;
        Ok(PreparedIterationItem {
            renderer: control.renderer,
            receipt: control.receipt.copy_held(),
            ordinal,
            batch_size: control.batch_size,
            value: iteration_value(control.value(), ordinal, control.batch_size)?,
        })
    }
}

fn control_phase(error: Error, phase: &str) -> Error {
    match error {
        Error::PermissionDenied(_) => {
            Error::PermissionDenied(format!("Iteration control {phase} denied"))
        }
        other => other,
    }
}

pub(super) fn with_items_path(path: &str) -> bool {
    let Some(task) = path
        .strip_prefix("/workflow/")
        .and_then(|path| path.strip_suffix("/with_items"))
    else {
        return false;
    };
    let tokens = task.split('/').collect::<Vec<_>>();
    !tokens.is_empty()
        && tokens.len() % 2 == 0
        && tokens
            .as_chunks::<2>()
            .0
            .iter()
            .all(|pair| pair[0] == "tasks" && !pair[1].is_empty())
}

pub(super) async fn storage_bytes(conn: &mut PgConnection, receipt: Id) -> Result<usize> {
    type IterationControlRow = (
        Id,
        Id,
        Id,
        String,
        Option<i64>,
        String,
        Option<String>,
        String,
        Option<String>,
    );
    let row:Option<IterationControlRow>=sqlx::query_as("SELECT binding.execution,binding.workflow,binding.workflow_definition,binding.task_path,binding.batch_size,binding.prepared_transaction,binding.publication_transaction,stage.prepared_transaction,stage.publication_transaction FROM runtime_native_iteration_control binding JOIN runtime_input_receipt receipt ON receipt.id=binding.receipt JOIN runtime_input_receipt_staged stage ON stage.receipt=receipt.id WHERE receipt.id=$1 AND receipt.entity_kind='workflow_variables' AND receipt.destination_purpose='control' AND receipt.execution=binding.execution AND receipt.destination_path=binding.task_path AND receipt.renderer_admission=receipt.destination_admission").bind(receipt).fetch_optional(&mut *conn).await?;
    let Some((
        execution,
        workflow,
        definition,
        path,
        batch,
        prepared,
        published,
        stage_prepared,
        stage_published,
    )) = row
    else {
        let missing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_iteration_control WHERE receipt=$1) OR EXISTS(SELECT 1 FROM runtime_input_receipt receipt JOIN runtime_input_receipt_staged stage ON stage.receipt=receipt.id WHERE receipt.id=$1 AND receipt.destination_purpose='control' AND receipt.destination_path LIKE '/workflow/tasks/%/with_items')").bind(receipt).fetch_one(conn).await?;
        return if missing { Err(denied()) } else { Ok(0) };
    };
    if execution <= 0
        || workflow <= 0
        || definition <= 0
        || !with_items_path(&path)
        || batch.is_some_and(|batch| batch <= 0)
        || prepared.is_empty()
        || stage_prepared != prepared
        || stage_published != published
        || published.as_ref().is_some_and(|value| value != &prepared)
        || (published.is_none() && transaction(conn).await? != prepared)
    {
        return Err(denied());
    }
    // Reserve the actual normalized owner/binding row and its final marker.
    Ok(serde_json::to_vec(&(
        Id::MAX,
        execution,
        workflow,
        definition,
        path,
        batch,
        &prepared,
        &prepared,
    ))?
    .len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_control_is_not_a_cache_selector_or_condition() {
        assert!(with_items_path("/workflow/tasks/0/with_items"));
        assert!(with_items_path(
            "/workflow/tasks/outer/tasks/inner/with_items"
        ));
        for path in [
            "/workflow/tasks/0/iterate_cache/namespace",
            "/workflow/tasks/0/next/0/when",
            "/workflow/tasks/with_items",
            "/workflow/tasks/0/with_items/0",
            "/workflow/tasks/0/params/x/with_items",
            "/workflow/tasks/0/next/0/with_items",
        ] {
            assert!(!with_items_path(path));
        }
    }
}
