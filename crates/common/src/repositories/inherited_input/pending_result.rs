//! Prospective ResultView VALUE/Condition facts. Construction requires actual
//! owning A tokens. Public readers still require both source and publication.
use super::super::native_input_snapshot::PreparedNativeResultView;
use super::*;

pub enum NativeResultStageBinding {
    Workflow,
    Vars,
    Variables,
    Direct,
}

pub struct PreparedNativeResultStage {
    workflow_original: Value,
    original_checked: std::sync::OnceLock<()>,
    staged: PreparedStagedValue,
    target_version: Value,
    target_binding: String,
}
impl PreparedNativeResultStage {
    pub fn receipt(&self) -> &ReceiptContext {
        &self.staged.receipt
    }
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
    pub fn ensure_fresh(&self) -> Result<()> {
        self.receipt().ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.receipt().ensure_fresh_on(conn).await
    }
}

pub struct PreparedNativeResultCondition {
    source: PreparedInputSource,
}
impl PreparedNativeResultCondition {
    pub fn source(&self) -> &PreparedInputSource {
        &self.source
    }
    pub fn value(&self) -> bool {
        self.source.receipt.value == Value::Bool(true)
    }
    pub fn receipt(&self) -> &ReceiptContext {
        &self.source.receipt
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.receipt().ensure_fresh()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.receipt().ensure_fresh_on(conn).await
    }
}

impl InheritedInputRepository {
    pub async fn prepare_native_result_stage(
        conn: &mut PgConnection,
        view: &PreparedNativeResultView,
        destination: &PreparedNativeRecipeDestination,
        key: &str,
    ) -> Result<PreparedNativeResultStage> {
        view.ensure_fresh_on(conn).await?;
        if destination.rendered.transaction != view.transaction
            || destination.rendered.renderer.id != view.renderer
            || !matches!(&destination.rendered.native, Some(NativeValueRecipe::ResultShorthand {snapshot,..}) if *snapshot==view.snapshot)
            || destination.rendered.value != view.value
            || !destination
                .rendered
                .result_reads
                .iter()
                .any(|read| read.snapshot() == view.snapshot)
        {
            return Err(denied());
        }
        // This owning destination already derives the immutable directive/name,
        // exact view and all firing/independent controls. Never accept a path.
        let receipt = Self::record_prepared_inner(
            conn,
            &destination.rendered,
            &destination.destination,
            destination.value(),
            destination.rendered.selections(),
            key,
            true,
        )
        .await?;
        mark_pending(conn, &receipt).await?;
        let target_version = native_values::target_version(conn, receipt.site.execution).await?;
        let target_binding =
            native_result_view::target_binding(conn, receipt.site.execution).await?;
        Ok(PreparedNativeResultStage {
            workflow_original: destination
                .workflow_original
                .as_ref()
                .ok_or_else(denied)?
                .clone(),
            original_checked: std::sync::OnceLock::new(),
            staged: PreparedStagedValue { receipt },
            target_version,
            target_binding,
        })
    }

    pub fn bind_native_result_stage(
        session: &mut RenderSession,
        stage: &PreparedNativeResultStage,
        mode: NativeResultStageBinding,
    ) -> Result<()> {
        let path = stage_binding_path(&stage.receipt().site.path, mode)?;
        Self::bind_staged_value(session, path, &stage.staged)
    }

    pub async fn finalize_native_result_stage(
        conn: &mut PgConnection,
        stage: &PreparedNativeResultStage,
        key: &str,
    ) -> Result<()> {
        stage.ensure_fresh_on(conn).await?;
        let target = stage.receipt().site.execution;
        if stage.original_checked.get().is_none()
            || native_values::target_version(conn, target).await? != stage.target_version
            || native_result_view::target_binding(conn, target).await? != stage.target_binding
        {
            return Err(denied());
        }
        // Includes every transitive native release and staged/control publication,
        // then actual persisted redaction, ciphertext, value and exact provenance.
        Self::finalize_staged_value(conn, &stage.staged, key).await
    }

    pub async fn prepare_native_result_condition(
        conn: &mut PgConnection,
        view: &PreparedNativeResultView,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        key: &str,
    ) -> Result<PreparedNativeResultCondition> {
        view.require_condition_scope(conn, &site.path).await?;
        if rendered.purpose != InputSitePurpose::Condition
            || site.purpose != InputSitePurpose::Condition
            || site.kind != InputEntityKind::WorkflowVariables
            || site.execution != rendered.renderer.owner_id
            || rendered.renderer.id != view.renderer
            || rendered.transaction != view.transaction
            || !rendered.value.is_boolean()
            || rendered.native.is_some()
            || rendered.literal.is_some()
        {
            return Err(denied());
        }
        let (body, expected): (Value,String) = sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2").bind(rendered.declaration.ok_or_else(denied)?).bind(rendered.renderer.policy_revision).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
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
        let mut reads = rendered.result_reads.clone();
        merge_result_reads(&mut reads, &[view.issued_result_read(&site.path)?])?;
        let pending = RenderedReceiptContext {
            result_reads: reads,
            empty_reads: rendered.empty_reads.clone(),
            wait_reads: rendered.wait_reads.clone(),
            native: None,
            transaction: rendered.transaction.clone(),
            renderer: rendered.renderer.clone(),
            declaration: rendered.declaration,
            literal: None,
            value: rendered.value.clone(),
            selections: rendered.selections.clone(),
            fence: rendered.fence.intersect(&view.fence),
            purpose: InputSitePurpose::Condition,
        };
        let destination = Self::prepare_destination(conn, &pending, site, key).await?;
        if !destination
            .native_sources
            .iter()
            .any(|source| source.snapshot() == view.snapshot)
        {
            return Err(denied());
        }
        let receipt = Self::record_prepared_inner(
            conn,
            &pending,
            &destination,
            &pending.value,
            &pending.selections,
            key,
            true,
        )
        .await?;
        mark_pending(conn, &receipt).await?;
        Ok(PreparedNativeResultCondition {
            source: PreparedInputSource {
                renderer: view.renderer,
                receipt,
                controls: Vec::new(),
            },
        })
    }

    pub async fn finalize_native_result_condition(
        conn: &mut PgConnection,
        condition: &PreparedNativeResultCondition,
    ) -> Result<()> {
        let receipt = condition.receipt();
        receipt.ensure_fresh_on(conn).await?;
        require_published_sources(conn, &receipt.source_receipts).await?;
        Self::require_finalized_native_sources(conn, &receipt.native_sources).await?;
        let changed = sqlx::query("UPDATE runtime_input_receipt_staged SET publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1 AND prepared_transaction=pg_current_xact_id()::TEXT AND publication_transaction IS NULL").bind(receipt.id).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        receipt.ensure_fresh_on(conn).await
    }
}

async fn mark_pending(conn: &mut PgConnection, receipt: &ReceiptContext) -> Result<()> {
    sqlx::query("INSERT INTO runtime_input_receipt_staged(receipt) VALUES($1)")
        .bind(receipt.id)
        .execute(conn)
        .await?;
    Ok(())
}

fn stage_binding_path(site_path: &str, mode: NativeResultStageBinding) -> Result<String> {
    let encoded = site_path
        .strip_prefix('/')
        .filter(|name| !name.is_empty() && !name.contains('/'))
        .ok_or_else(denied)?;
    let name = encoded.replace("~1", "/").replace("~0", "~");
    if escape(&name) != encoded {
        return Err(denied());
    }
    let namespace = match mode {
        NativeResultStageBinding::Workflow => "workflow",
        NativeResultStageBinding::Vars => "vars",
        NativeResultStageBinding::Variables => "variables",
        NativeResultStageBinding::Direct => {
            if protected_namespace(site_path)
                || matches!(
                    name.as_str(),
                    "parameters"
                        | "workflow"
                        | "vars"
                        | "variables"
                        | "task"
                        | "tasks"
                        | "result"
                        | "config"
                        | "pack"
                        | "keystore"
                        | "system"
                        | "item"
                        | "index"
                        | "inquiry"
                        | "execution"
                        | "work_queue_item"
                )
            {
                return Err(denied());
            }
            return Ok(site_path.into());
        }
    };
    Ok(format!("/{namespace}{site_path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_names_come_only_from_the_issued_single_segment_site() {
        assert_eq!(
            stage_binding_path("/whole", NativeResultStageBinding::Workflow).unwrap(),
            "/workflow/whole"
        );
        assert_eq!(
            stage_binding_path("/a~1b~0c", NativeResultStageBinding::Variables).unwrap(),
            "/variables/a~1b~0c"
        );
        assert_eq!(
            stage_binding_path("/whole", NativeResultStageBinding::Vars).unwrap(),
            "/vars/whole"
        );
        assert_eq!(
            stage_binding_path("/whole", NativeResultStageBinding::Direct).unwrap(),
            "/whole"
        );
        for invalid in ["whole", "", "/", "/whole/token", "/bad~2name"] {
            assert!(stage_binding_path(invalid, NativeResultStageBinding::Workflow).is_err());
        }
    }

    #[test]
    fn direct_alias_cannot_replace_canonical_evaluator_namespaces() {
        for name in [
            "parameters",
            "workflow",
            "vars",
            "variables",
            "task",
            "tasks",
            "result",
            "config",
            "pack",
            "keystore",
            "system",
            "item",
            "index",
            "inquiry",
            "execution",
            "work_queue_item",
            "$outcome",
        ] {
            assert!(
                stage_binding_path(&format!("/{name}"), NativeResultStageBinding::Direct).is_err()
            );
        }
        // A reserved variable name remains usable inside the actual variable namespace.
        assert_eq!(
            stage_binding_path("/result", NativeResultStageBinding::Workflow).unwrap(),
            "/workflow/result"
        );
    }
}
