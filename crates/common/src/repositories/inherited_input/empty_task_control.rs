//! Ordinary task metadata is CONTROL, not a published variable or iteration list.
use super::*;

pub(crate) struct PreparedEmptyTaskControl {
    receipt: ReceiptContext,
    destination: PreparedReceiptDestination,
}

impl PreparedEmptyTaskControl {
    pub(crate) fn receipt(&self) -> &ReceiptContext {
        &self.receipt
    }
    pub(crate) fn materialized_bytes(&self) -> Result<usize> {
        let graph = self.receipt.graph_accounting.as_ref().ok_or_else(denied)?;
        graph
            .bytes
            .checked_add(serde_json::to_vec(self.receipt.accepted_value())?.len())
            .ok_or_else(denied)
    }
    pub(crate) fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok([
            self.destination.held_deadline()?,
            self.receipt.approval_deadline(),
        ]
        .into_iter()
        .flatten()
        .min())
    }
    pub(crate) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.destination.ensure_fresh_on(conn).await?;
        self.receipt.ensure_fresh_on(conn).await
    }
    pub(crate) fn ensure_fresh(&self) -> Result<()> {
        self.destination.ensure_fresh()?;
        self.receipt.ensure_fresh()
    }
    /// Only after the owning empty frame has published. No rerender or graph
    /// reopening behind target locks; consume the exact held source descriptors.
    pub(crate) async fn publish(&self, conn: &mut PgConnection, child: Id) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        InheritedInputRepository::require_finalized_native_sources(
            conn,
            &self.receipt.native_sources,
        )
        .await?;
        require_published_sources(conn, &self.receipt.source_receipts).await?;
        let changed = sqlx::query("UPDATE runtime_native_empty_task_control SET child=$2,publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1 AND prepared_transaction=pg_current_xact_id()::TEXT AND publication_transaction IS NULL AND child IS NULL")
            .bind(self.receipt.id).bind(child).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        let changed = sqlx::query("UPDATE runtime_input_receipt_staged SET publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1 AND prepared_transaction=pg_current_xact_id()::TEXT AND publication_transaction IS NULL")
            .bind(self.receipt.id).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await
    }
}

impl InheritedInputRepository {
    pub(crate) async fn prepare_empty_task_control(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        frame: &super::super::native_transition_materialization::PreparedNativeEmptyTransitions,
        task: &str,
        key: &str,
    ) -> Result<PreparedEmptyTaskControl> {
        require_transaction(conn, &rendered.transaction).await?;
        frame.require_source_current(conn).await?;
        let source = frame.source_outcome()?;
        let task_path = frame.target_path(task)?;
        if rendered.purpose != InputSitePurpose::Control
            || rendered.native.is_some()
            || rendered.renderer.owner_kind != "execution"
            || site.execution != rendered.renderer.owner_id
            || site.kind != InputEntityKind::WorkflowVariables
            || site.purpose != InputSitePurpose::Control
            || rendered.literal.is_some()
            || rendered.empty_reads.is_empty()
            || !metadata_path(&site.path)
            || !site
                .path
                .strip_prefix(task_path)
                .is_some_and(|suffix| suffix.starts_with('/'))
            || !rendered
                .empty_reads
                .iter()
                .any(|read| read.snapshot() == source.snapshot())
        {
            return Err(denied());
        }
        let (body, expected): (Value, String) = sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2")
            .bind(rendered.declaration.ok_or_else(denied)?).bind(rendered.renderer.policy_revision)
            .fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        bounded(&body)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let declaration: super::super::declared_input::DeclaredInputBinding =
            serde_json::from_value(body)?;
        DeclaredInputRepository::validate_declaration(conn, &rendered.renderer, &declaration)
            .await?;
        if declaration.source_path != site.path {
            return Err(denied());
        }
        let destination = Self::prepare_destination(conn, rendered, site, key).await?;
        let receipt = Self::record_prepared_inner(
            conn,
            rendered,
            &destination,
            rendered.value(),
            rendered.selections(),
            key,
            true,
        )
        .await?;
        sqlx::query("INSERT INTO runtime_input_receipt_staged(receipt) VALUES($1)")
            .bind(receipt.id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("INSERT INTO runtime_native_empty_task_control(receipt,execution,workflow,workflow_definition,source_snapshot,task_name,task_path) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(receipt.id).bind(site.execution).bind(frame.workflow().id).bind(frame.workflow().workflow_def)
            .bind(source.snapshot()).bind(task).bind(task_path).execute(&mut *conn).await?;
        let mut receipt = Self::preparation_receipt_context_with_admissions(
            conn,
            site,
            &receipt.origin(),
            key,
            &receipt.wait_reads,
            &receipt.result_reads,
            &receipt.empty_reads,
            &mut RetainedReceiptAdmissions::default(),
        )
        .await?;
        receipt.newly_recorded = true;
        Ok(PreparedEmptyTaskControl {
            receipt,
            destination,
        })
    }
}

pub(super) async fn storage_bytes(conn: &mut PgConnection, receipt: Id) -> Result<usize> {
    type EmptyTaskControlRow = (
        Id,
        Id,
        Id,
        Id,
        String,
        String,
        Option<Id>,
        String,
        Option<String>,
        String,
        Option<String>,
    );
    let row: Option<EmptyTaskControlRow> = sqlx::query_as("SELECT binding.execution,binding.workflow,binding.workflow_definition,binding.source_snapshot,binding.task_name,binding.task_path,binding.child,binding.prepared_transaction,binding.publication_transaction,stage.prepared_transaction,stage.publication_transaction FROM runtime_native_empty_task_control binding JOIN runtime_input_receipt receipt ON receipt.id=binding.receipt JOIN runtime_input_receipt_staged stage ON stage.receipt=receipt.id JOIN runtime_native_input_snapshot source ON source.id=binding.source_snapshot WHERE receipt.id=$1 AND receipt.entity_kind='workflow_variables' AND receipt.destination_purpose='control' AND receipt.execution=binding.execution AND receipt.renderer_admission=receipt.destination_admission AND source.renderer_admission=receipt.renderer_admission AND source.body#>>'{source,kind}'='empty_iteration' AND source.body#>>'{source,execution}'=binding.execution::TEXT AND source.body#>>'{source,workflow}'=binding.workflow::TEXT AND source.body#>>'{source,definition}'=binding.workflow_definition::TEXT AND source.body#>>'{source,transaction}'=binding.prepared_transaction")
        .bind(receipt).fetch_optional(&mut *conn).await?;
    let Some((
        execution,
        workflow,
        definition,
        source,
        task,
        path,
        child,
        prepared,
        published,
        stage_prepared,
        stage_published,
    )) = row
    else {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_native_empty_task_control WHERE receipt=$1)",
        )
        .bind(receipt)
        .fetch_one(conn)
        .await?;
        return if exists { Err(denied()) } else { Ok(0) };
    };
    if execution <= 0
        || workflow <= 0
        || definition <= 0
        || source <= 0
        || task.is_empty()
        || !path.starts_with("/workflow/tasks/")
        || prepared.is_empty()
        || stage_prepared != prepared
        || stage_published != published
        || published.as_ref().is_some_and(|value| value != &prepared)
        || (published.is_none() && (child.is_some() || transaction(conn).await? != prepared))
        || (published.is_some() && child.is_none_or(|child| child <= 0))
    {
        return Err(denied());
    }
    Ok(serde_json::to_vec(&(
        Id::MAX,
        execution,
        workflow,
        definition,
        source,
        task,
        path,
        Id::MAX,
        &prepared,
        &prepared,
    ))?
    .len())
}

fn metadata_path(path: &str) -> bool {
    let Some(path) = path.strip_prefix("/workflow/") else {
        return false;
    };
    let tokens: Vec<_> = path.split('/').collect();
    let mut i = 0;
    while tokens.get(i) == Some(&"tasks") && tokens.get(i + 1).is_some_and(|name| !name.is_empty())
    {
        i += 2;
    }
    i > 0
        && matches!(
            tokens.get(i).copied(),
            Some(
                "action"
                    | "permission_set_refs"
                    | "timeout"
                    | "worker_selector"
                    | "worker_tolerations"
                    | "worker_affinity"
                    | "trace_tag_template"
            )
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_is_not_value_condition_or_iteration_control() {
        assert!(metadata_path("/workflow/tasks/0/action"));
        assert!(metadata_path(
            "/workflow/tasks/a/tasks/b/worker_selector/zone"
        ));
        for path in [
            "/workflow/tasks/0/input/x",
            "/workflow/tasks/0/with_items",
            "/workflow/tasks/0/next/0/when",
            "/workflow/tasks/action",
            "/workflow/tasks/0/iterate_cache/action",
        ] {
            assert!(!metadata_path(path), "{path}");
        }
    }
}
