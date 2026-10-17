//! Private composition of actual iterate_cache selector declarations. Boolean
//! transition decisions and VALUE staging keep their separate entry points.
use super::*;

#[derive(Clone)]
pub(crate) struct CacheControlPublication {
    native_sources: Vec<super::super::native_input_snapshot::NativeSnapshotMetadata>,
    receipt: Id,
    transaction: String,
    sources: Vec<ReceiptSourceDescriptor>,
    fence: ApprovalEffectFence,
}

impl CacheControlPublication {
    /// Behind targets, consume only already-held proof and normalized publication
    /// facts. Never reopen a source graph or acquire producer/current IAM rights.
    pub(crate) async fn require_ready(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await?;
        InheritedInputRepository::require_finalized_native_sources(conn, &self.native_sources)
            .await?;
        require_published_sources(conn, &self.sources).await
    }

    pub(crate) async fn publish(&self, conn: &mut PgConnection, snapshot: Id) -> Result<()> {
        self.require_ready(conn).await?;
        let associated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_parent parent JOIN runtime_native_cache_frontier frontier ON frontier.snapshot=parent.snapshot WHERE parent.receipt=$1 AND parent.snapshot=$2 AND frontier.bound_transaction=pg_current_xact_id()::TEXT)")
            .bind(self.receipt).bind(snapshot).fetch_one(&mut *conn).await?;
        if !associated {
            return Err(denied());
        }
        let published: Option<String> = sqlx::query_scalar("SELECT publication_transaction FROM runtime_input_receipt_staged WHERE receipt=$1 AND prepared_transaction=pg_current_xact_id()::TEXT")
            .bind(self.receipt).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        if published.as_deref() == Some(self.transaction.as_str()) {
            return Ok(());
        }
        if published.is_some() {
            return Err(denied());
        }
        let changed=sqlx::query("UPDATE runtime_input_receipt_staged SET publication_transaction=pg_current_xact_id()::TEXT WHERE receipt=$1 AND prepared_transaction=pg_current_xact_id()::TEXT AND publication_transaction IS NULL")
            .bind(self.receipt).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await
    }
}

impl InheritedInputRepository {
    async fn held_graph_accounting(
        conn: &mut PgConnection,
        context: &ReceiptContext,
        key: &str,
        admissions: &mut RetainedReceiptAdmissions,
    ) -> Result<std::sync::Arc<ReceiptGraphAccounting>> {
        context.ensure_fresh_on(conn).await?;
        if let Some(accounting) = &context.graph_accounting {
            if accounting.key_hash != crate::crypto::hash_encryption_key(key) {
                return Err(denied());
            }
            return Ok(accounting.clone());
        }
        // Newly recorded receipts have not passed the canonical graph reader.
        // Authenticate those once, carrying only their genuine issued reads.
        let (read, _, _) = Self::counted_preparation_receipt_context(
            conn,
            &context.site,
            &context.origin(),
            key,
            &context.wait_reads,
            &context.result_reads,
            admissions,
        )
        .await?;
        read.graph_accounting.ok_or_else(denied)
    }
    pub(crate) async fn require_cache_selector_publications(
        conn: &mut PgConnection,
        parents: &[(Id, InputSite)],
    ) -> Result<()> {
        let ids = parents
            .iter()
            .map(|(receipt, _)| *receipt)
            .collect::<Vec<_>>();
        let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_input_receipt_staged WHERE receipt=ANY($1) AND publication_transaction IS NULL)").bind(&ids).fetch_one(conn).await?;
        if pending {
            return Err(denied());
        }
        Ok(())
    }
    pub(crate) async fn require_cache_selector_bounds(
        conn: &mut PgConnection,
        contexts: &[&ReceiptContext],
        key: &str,
    ) -> Result<usize> {
        // Storage inventory requires published Wait releases. During source
        // preparation consume the canonical graph reader's accounting instead,
        // using only the genuine issued reads already carried by each Control.
        // It charges receipts, accepted inputs and native archive reservations.
        // Publication still requires require_ready and the actual owner seal.
        let mut bytes = 0usize;
        let mut admissions = RetainedReceiptAdmissions::default();
        for context in contexts {
            let accounting =
                Self::held_graph_accounting(conn, context, key, &mut admissions).await?;
            bytes = bytes.checked_add(accounting.bytes).ok_or_else(denied)?;
        }
        if bytes > 1024 * 1024 {
            return Err(denied());
        }
        Ok(bytes)
    }

    /// Source-phase accounting accepts held capabilities, never receipt IDs or
    /// an allow-unpublished flag. The canonical reader validates every edge
    /// using the original carried reads and transaction before charging it.
    pub(crate) async fn prepared_source_graph_bounds(
        conn: &mut PgConnection,
        contexts: &[&ReceiptContext],
        sources: &[PreparedInputSource],
        key: &str,
    ) -> Result<(usize, usize)> {
        let mut bytes = 0usize;
        let mut nodes = BTreeSet::new();
        let mut admissions = RetainedReceiptAdmissions::default();
        for context in contexts
            .iter()
            .copied()
            .chain(sources.iter().map(|source| &source.receipt))
        {
            let accounting =
                Self::held_graph_accounting(conn, context, key, &mut admissions).await?;
            bytes = bytes.checked_add(accounting.bytes).ok_or_else(denied)?;
            nodes.extend(accounting.nodes.iter().copied());
            // Reserve one node for the owning EOF snapshot.
            if bytes > 1024 * 1024 || nodes.len() >= 256 {
                return Err(denied());
            }
        }
        Ok((bytes, nodes.len()))
    }
    /// Issue an opaque Control only from the canonical single evaluation of a
    /// retained iterate_cache selector. This is not authored/literal input and
    /// does not publish a workflow variable, a Boolean decision or a reader grant.
    pub async fn prepare_control_source(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        key: &str,
    ) -> Result<PreparedInputSource> {
        require_transaction(conn, &rendered.transaction).await?;
        if rendered.purpose != InputSitePurpose::Control
            || site.kind != InputEntityKind::WorkflowVariables
            || site.purpose != InputSitePurpose::Control
            || site.execution != rendered.renderer.owner_id
            || !cache_selector_path(&site.path)
            || rendered.literal.is_some()
        {
            return Err(denied());
        }
        let (body, expected):(Value,String)=sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2")
            .bind(rendered.declaration.ok_or_else(denied)?).bind(rendered.renderer.policy_revision).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        bounded(&body)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let declaration: super::super::declared_input::DeclaredInputBinding =
            serde_json::from_value(body)?;
        DeclaredInputRepository::validate_declaration(conn, &rendered.renderer, &declaration)
            .await?;
        if declaration.source_path != site.path || declaration.expression.is_empty() {
            return Err(denied());
        }
        let prepared = Self::prepare_destination(conn, rendered, site, key).await?;
        let receipt = Self::record_prepared_inner(
            conn,
            rendered,
            &prepared,
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
        // J's canonical private graph reader authenticates pending ancestry in
        // this issuing transaction and enforces the existing cumulative bounds.
        let mut receipt = Self::preparation_receipt_context_with_wait_reads(
            conn,
            &receipt.site,
            &receipt.origin(),
            key,
            &receipt.wait_reads,
        )
        .await?;
        receipt.newly_recorded = true;
        Ok(PreparedInputSource {
            renderer: rendered.renderer.id,
            receipt,
            controls: vec![],
        })
    }
}

impl PreparedInputSource {
    pub(crate) async fn cache_selector_context(
        &self,
        conn: &mut PgConnection,
        renderer: Id,
    ) -> Result<(ReceiptContext, CacheControlPublication)> {
        let receipt = &self.receipt;
        require_transaction(conn, &receipt.transaction).await?;
        if self.renderer != renderer
            || !receipt.newly_recorded
            || receipt.site.kind != InputEntityKind::WorkflowVariables
            || receipt.site.purpose != InputSitePurpose::Control
            || !cache_selector_path(&receipt.site.path)
            || !self.controls.is_empty()
        {
            return Err(denied());
        }
        receipt
            .require_declared_control(conn, renderer, &receipt.site.path)
            .await?;
        let issued:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_input_receipt_staged WHERE receipt=$1 AND prepared_transaction=pg_current_xact_id()::TEXT AND publication_transaction IS NULL)")
            .bind(receipt.id).fetch_one(&mut *conn).await?;
        if !issued {
            return Err(denied());
        }
        Ok((
            receipt.copy_held(),
            CacheControlPublication {
                native_sources: receipt.native_sources.clone(),
                receipt: receipt.id,
                transaction: receipt.transaction.clone(),
                sources: receipt.source_receipts.clone(),
                fence: receipt.fence.clone(),
            },
        ))
    }
}

fn cache_selector_path(path: &str) -> bool {
    path.rsplit_once("/iterate_cache/")
        .is_some_and(|(task, field)| {
            task.starts_with("/workflow/tasks/")
                && !task.is_empty()
                && matches!(
                    field,
                    "namespace" | "generation" | "owner_type" | "owner_ref"
                )
        })
}

pub(super) async fn staged_storage_bytes(conn: &mut PgConnection, receipt: Id) -> Result<usize> {
    let row:Option<(String,Option<String>)>=sqlx::query_as("SELECT prepared_transaction,publication_transaction FROM runtime_input_receipt_staged WHERE receipt=$1").bind(receipt).fetch_optional(&mut *conn).await?;
    let staged = row
        .map(|(prepared, published)| {
            if published
                .as_ref()
                .is_some_and(|published| published != &prepared)
            {
                return Err(denied());
            }
            // Reserve the final publication string even while the Control is pending.
            Ok(serde_json::to_vec(&(Id::MAX, &prepared, &prepared))?.len())
        })
        .transpose()
        .map(|charge| charge.unwrap_or(0))?;
    staged
        .checked_add(super::iteration_control::storage_bytes(conn, receipt).await?)
        .ok_or_else(denied)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selector_controls_are_not_transition_decisions_or_value_sites() {
        for field in ["namespace", "generation", "owner_type", "owner_ref"] {
            assert!(cache_selector_path(&format!(
                "/workflow/tasks/0/iterate_cache/{field}"
            )));
        }
        for path in [
            "/workflow/tasks/0/next/0/when",
            "/workflow/tasks/0/publish/namespace",
            "/workflow/tasks/0/iterate_cache/require_fresh",
            "/parameters/namespace",
            "/workflow/tasks/0/iterate_cache/namespace/value",
        ] {
            assert!(!cache_selector_path(path));
        }
    }
}
