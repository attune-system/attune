//! A released Wait output owns a root and a separate original Wait observer.
use super::super::{
    execution_secret_value::SecretProvenanceRow,
    inherited_input::{
        InheritedInputRepository as Inputs, InputSite, InputSitePurpose, ReceiptSourceDescriptor,
    },
    runtime_authority::{ApprovalEffectFence, RuntimeAuthorityRepository},
};
use super::success::FiniteExecutionSource;
use super::*;

pub struct PreparedWaitCompletionSource {
    metadata: NativeSnapshotMetadata,
    evidence: NativeSourceEvidence,
    root: FiniteExecutionSource,
    original: PreparedWaitSource,
    fields: Vec<SecretProvenanceRow>,
    closure: SecretProvenanceRow,
    parents: Vec<RuntimeReadParent>,
    receipts: Vec<(Id, InputSite)>,
    original_receipts: Vec<ReceiptContext>,
    program: Vec<ReceiptContext>,
    fence: ApprovalEffectFence,
    transaction: String,
    nodes: usize,
    bytes: usize,
    depth: usize,
}
impl PreparedWaitCompletionSource {
    pub fn metadata(&self) -> &NativeSnapshotMetadata {
        &self.metadata
    }
    pub fn root(&self) -> &FiniteExecutionSource {
        &self.root
    }
    pub fn original(&self) -> &PreparedWaitSource {
        &self.original
    }
    pub fn original_snapshot(&self) -> Id {
        self.original.metadata().snapshot()
    }
    pub fn parents(&self) -> &[RuntimeReadParent] {
        &self.parents
    }
    pub fn retained_nodes(&self) -> usize {
        self.nodes
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub fn approval_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.fence.approval_deadline()
    }
    pub fn scope_node(&self) -> (String, Id, String) {
        (
            "wait_completion_metadata".into(),
            self.metadata.snapshot(),
            String::new(),
        )
    }
    fn is_root(&self, origin: &SecretOrigin) -> bool {
        matches!(origin,SecretOrigin::Entity{entity_type,entity_id,path} if entity_type=="wait_completion_metadata"&&*entity_id==self.metadata.snapshot()&&path.is_empty())
    }
    fn is_field(&self, origin: &SecretOrigin) -> bool {
        *origin == self.original.root()
            || matches!(origin,SecretOrigin::Entity{entity_type,entity_id,path} if self.fields.iter().any(|row|row.entity_type==*entity_type&&row.entity_id==*entity_id&&row.json_path==*path))
    }
    pub(crate) fn covers_receipt(&self, source: &ReceiptSourceDescriptor) -> bool {
        self.receipts
            .iter()
            .any(|(id, site)| *id == source.receipt() && site == source.site())
    }
    pub fn rows_for(&self, origin: &SecretOrigin) -> Result<Vec<SecretProvenanceRow>> {
        self.ensure_fresh()?;
        if self.is_root(origin) {
            let mut rows = self.fields.clone();
            rows.push(self.closure.clone());
            return Ok(rows);
        }
        let SecretOrigin::Entity {
            entity_type,
            entity_id,
            path,
        } = origin
        else {
            return Err(denied());
        };
        if *origin == self.original.root() {
            return Ok(self
                .fields
                .iter()
                .filter(|row| row.entity_type == *entity_type && row.entity_id == *entity_id)
                .cloned()
                .collect());
        }
        Ok(vec![self
            .fields
            .iter()
            .find(|row| {
                row.entity_type == *entity_type
                    && row.entity_id == *entity_id
                    && row.json_path == *path
            })
            .ok_or_else(denied)?
            .clone()])
    }
    pub fn row_site(&self, row: &SecretProvenanceRow) -> Result<Option<InputSite>> {
        if same_row(row, &self.closure) {
            return Ok(None);
        }
        if !self.fields.iter().any(|saved| same_row(saved, row)) {
            return Err(denied());
        }
        // Inquiry/queue observation fields are not execution VALUE deliveries.
        // Only the exact authenticated original observation may lack a site.
        if matches!(
            self.original.metadata().source_kind(),
            "inquiry_response" | "queue_item_result"
        ) && row.entity_type == self.original.metadata().source_kind()
            && row.entity_id == self.original.metadata().target().id
        {
            return Ok(None);
        }
        Ok(Some(
            super::super::immutable_input_origin::input_value_site(
                &row.entity_type,
                row.entity_id,
                &row.json_path,
            )?,
        ))
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        self.original.ensure_fresh()?;
        for context in self.original_receipts.iter().chain(&self.program) {
            context.ensure_fresh()?;
        }
        Ok(())
    }
    pub(crate) async fn ensure_held_on(&self, conn: &mut PgConnection) -> Result<()> {
        let current: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if current != self.transaction {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await?;
        self.original.ensure_held_on(conn).await?;
        for context in self.original_receipts.iter().chain(&self.program) {
            context.ensure_fresh_on(conn).await?;
        }
        self.ensure_fresh()
    }
    pub fn program_context_count(&self) -> usize {
        self.program.len()
    }
    pub fn original_wait(&self) -> Result<Id> {
        Ok(self
            .evidence
            .wait_completion()
            .ok_or_else(denied)?
            .original_wait())
    }
}
impl NativeWaitSources {
    pub fn wait_completion(
        &self,
        metadata: &[NativeSnapshotMetadata],
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<PreparedWaitCompletionSource>>> {
        let mut found: Option<Arc<PreparedWaitCompletionSource>> = None;
        for item in metadata
            .iter()
            .filter(|m| m.source_kind() == "wait_completion_metadata")
        {
            let source = self
                .wait_completions
                .get(&item.snapshot())
                .ok_or_else(denied)?;
            source.ensure_fresh()?;
            if source.metadata() != item {
                return Err(denied());
            }
            if !source.is_root(origin) && !source.is_field(origin) {
                continue;
            }
            if found.as_ref().is_some_and(|saved| saved.metadata() != item) {
                return Err(denied());
            }
            found = Some(Arc::clone(source));
        }
        Ok(found)
    }
    pub fn wait_completion_in_scope(
        &self,
        ancestry: &BTreeSet<(String, Id, String)>,
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<PreparedWaitCompletionSource>>> {
        let mut found: Option<Arc<PreparedWaitCompletionSource>> = None;
        for source in self
            .wait_completions
            .values()
            .filter(|s| ancestry.contains(&s.scope_node()))
        {
            source.ensure_fresh()?;
            if !source.is_field(origin) {
                continue;
            }
            if found
                .as_ref()
                .is_some_and(|saved| saved.metadata() != source.metadata())
            {
                return Err(denied());
            }
            found = Some(Arc::clone(source));
        }
        Ok(found)
    }
    pub(super) async fn prepare_wait_completion(
        &mut self,
        conn: &mut PgConnection,
        context: &ReceiptContext,
        metadata: &NativeSnapshotMetadata,
        key: &str,
        depth: usize,
    ) -> Result<(usize, usize)> {
        if let Some(saved) = self.wait_completions.get(&metadata.snapshot()) {
            if saved.metadata() != metadata || depth.saturating_add(saved.depth) >= 32 {
                return Err(denied());
            }
            saved.ensure_held_on(conn).await?;
            return Ok((0, 0));
        }
        if self.source_count() >= 256 {
            return Err(denied());
        }
        lock_snapshot_archive(conn, metadata.snapshot()).await?;
        // These APIs require both normalized releases and the real program
        // publication seal. There is no pending flag or live-Wait-row fallback.
        let evidence = NativeInputSnapshotRepository::source_evidence(conn, metadata, key).await?;
        let storage =
            NativeInputSnapshotRepository::storage_evidence(conn, metadata.snapshot()).await?;
        let source = evidence.wait_completion().ok_or_else(denied)?;
        let stored = storage.wait_completion().ok_or_else(denied)?;
        let root_origin = SecretOrigin::Entity {
            entity_type: "wait_completion_metadata".into(),
            entity_id: metadata.snapshot(),
            path: String::new(),
        };
        if metadata.source_kind() != "wait_completion_metadata"
            || source.snapshot() != metadata.snapshot()
            || stored.snapshot() != metadata.snapshot()
            || source.execution() != stored.execution()
            || source.original_snapshot() != stored.original_snapshot()
            || source.transaction() != stored.transaction()
            || source.target().id != source.execution()
            || evidence.renderer_admission() != source.original_renderer_admission()
            || metadata.target() != source.target()
            || metadata.read_owner() != source.read_owner()
            || metadata.generation().is_some()
            || metadata.source_site() != Some(("execution_result", source.execution(), ""))
            || metadata.result_contract_kind().is_some()
            || metadata.result_pointer_mappings().is_some()
            || !evidence.fields().is_empty()
            || !storage.archive_fields().is_empty()
            || !evidence.contributor_manifests().is_empty()
            || !storage.contributor_manifests().is_empty()
            || !evidence.completion_sources().is_empty()
            || !storage.completion_sources().is_empty()
            || storage.wait_release().is_some()
            || storage.completion_release().is_some()
            || storage.result_view_release().is_some()
            || !evidence.origins().contains(&root_origin)
            || !context.source_origins().contains(&root_origin)
            || serde_json::to_value(evidence.contract())?
                != serde_json::to_value(source.original_output_contract().output.as_ref())?
            || serde_json::to_value(storage.contract())?
                != serde_json::to_value(source.original_output_contract().output.as_ref())?
            || source.original_parent().id != source.execution()
            || source.original_workflow().execution != source.execution()
            || source.original_workflow().id != source.workflow()
        {
            return Err(Error::PermissionDenied(
                "Wait output reader metadata identity mismatch".into(),
            ));
        }
        source.original_output_contract().validate()?;
        let registration = source.original_registration();
        if registration.renderer_admission() != source.original_renderer_admission()
            || registration.task_path() != source.original_task_path()
            || registration.transaction().is_empty()
            || registration.original_recipe().is_null()
        {
            return Err(Error::PermissionDenied(
                "Wait output reader registration mismatch".into(),
            ));
        }
        let (root, root_fence) = super::success::execution_target(
            conn,
            source.target(),
            source.read_owner(),
            evidence.renderer_admission(),
        )
        .await?;
        let original_metadata = source.original_metadata();
        if original_metadata.snapshot() != source.original_snapshot()
            || original_metadata.snapshot() == metadata.snapshot()
            || original_metadata.target() != source.original_target()
            || original_metadata.read_owner() != source.original_read_owner()
            || original_metadata.source_site() != Some(source.original_source_site())
        {
            return Err(Error::PermissionDenied(
                "Wait output reader original metadata mismatch".into(),
            ));
        }
        lock_execution_wait_snapshot(conn, original_metadata.snapshot()).await?;
        let original_evidence =
            NativeInputSnapshotRepository::source_evidence(conn, &original_metadata, key).await?;
        let original_storage =
            NativeInputSnapshotRepository::storage_evidence(conn, original_metadata.snapshot())
                .await?;
        let released = original_storage.wait_release().ok_or_else(denied)?;
        if released.wait() != source.original_wait()
            || released.snapshot() != source.original_snapshot()
            || released.transaction() != source.original_release_transaction()
            || original_evidence.renderer_admission() != source.original_renderer_admission()
            || original_evidence.source_admissions() != [source.original_source_admission()]
            || original_evidence.fields() != source.original_fields().as_slice()
            || original_evidence.origins() != source.original_origins()
            || serde_json::to_value(original_evidence.contributor_manifests())?
                != serde_json::to_value(source.contributor_manifests())?
            || serde_json::to_value(original_evidence.contract())?
                != serde_json::to_value(source.original_source_contract())?
        {
            return Err(Error::PermissionDenied(
                "Wait output reader original evidence mismatch".into(),
            ));
        }
        let selected =
            RuntimeAuthorityRepository::selected_effect_fence(conn, source.original_authority())
                .await?;
        let source_fence = RuntimeAuthorityRepository::retained_admission_fence(
            conn,
            source.original_source_admission(),
        )
        .await?;
        let renderer_fence = RuntimeAuthorityRepository::retained_admission_fence(
            conn,
            source.original_renderer_admission(),
        )
        .await?;
        let mut original =
            prepare_wait_source(conn, &original_metadata, original_evidence, context)
                .await
                .map_err(|error| match error {
                    Error::PermissionDenied(message) => {
                        Error::PermissionDenied(format!("Wait output original observer: {message}"))
                    }
                    other => other,
                })?;
        original.fence = original
            .fence
            .intersect(&selected)
            .intersect(&source_fence)
            .intersect(&renderer_fence);
        let mut fence = context
            .accepted_fence()
            .intersect(&root_fence)
            .intersect(&original.fence);
        let mut fields = BTreeMap::new();
        let mut root_ordinary = Vec::new();
        for (kind, values) in [
            ("execution_config", source.original_config_fields()),
            ("execution_result", source.original_result_fields()),
            ("workflow_variables", source.original_variable_fields()),
        ] {
            for field in values {
                super::success::collect_field(
                    &mut fields,
                    kind,
                    source.execution(),
                    &field,
                    &mut root_ordinary,
                )?;
            }
        }
        // The Wait-output root intentionally has no contributor-manifest union.
        // Canonical sealed root fields carry their own receipt ceilings. A raw
        // legacy ordinary root origin cannot borrow the original observer's
        // contributors or the producer's selected permissions instead.
        if !root_ordinary.is_empty() {
            return Err(Error::PermissionDenied(
                "Wait output reader root has unsealed origins".into(),
            ));
        }
        let mut original_ordinary = Vec::new();
        for field in source.original_fields() {
            super::success::collect_field(
                &mut fields,
                original_metadata.source_kind(),
                original_metadata.target().id,
                &field,
                &mut original_ordinary,
            )?;
        }
        if original_ordinary
            .iter()
            .any(|origin| !source.original_origins().contains(origin))
        {
            return Err(Error::PermissionDenied(
                "Wait output reader original origin mismatch".into(),
            ));
        }
        let receipts = source.receipt_edges().to_vec();
        if source
            .original_receipt_edges()
            .iter()
            .any(|edge| !receipts.contains(edge))
            || source
                .controls()
                .iter()
                .any(|edge| edge.1.purpose == InputSitePurpose::Value || !receipts.contains(edge))
            || source
                .issued_values()
                .iter()
                .any(|edge| edge.1.purpose != InputSitePurpose::Value || !receipts.contains(edge))
            || receipts.iter().any(|edge| {
                !source.original_receipt_edges().contains(edge)
                    && !source.controls().contains(edge)
                    && !source.issued_values().contains(edge)
            })
        {
            return Err(Error::PermissionDenied(
                "Wait output reader program receipt mismatch".into(),
            ));
        }
        let mut program = Vec::new();
        let mut original_receipts = Vec::new();
        let mut nodes = original
            .retained_nodes()
            .checked_add(1)
            .ok_or_else(denied)?;
        let mut bytes = evidence
            .retained_bytes()
            .checked_add(original.retained_bytes())
            .ok_or_else(denied)?;
        let mut closure_origins = vec![original.root()];
        for (id, site) in &receipts {
            let descriptor = context
                .source_receipts()
                .iter()
                .find(|d| d.receipt() == *id && d.site() == site)
                .ok_or_else(denied)?;
            let origin = descriptor.origin();
            if !closure_origins.contains(&origin) {
                closure_origins.push(origin);
            }
        }
        for (id, site) in source.original_receipt_edges() {
            let descriptor = context
                .source_receipts()
                .iter()
                .find(|d| d.receipt() == *id && d.site() == site)
                .ok_or_else(denied)?;
            let held = Inputs::prepare_receipt_source(conn, descriptor, key).await?;
            fence = fence.intersect(held.accepted_fence());
            let (held_bytes, held_nodes) =
                Inputs::prepared_source_graph_bounds(conn, &[&held], &[], key).await?;
            bytes = bytes.checked_add(held_bytes).ok_or_else(denied)?;
            nodes = nodes.checked_add(held_nodes).ok_or_else(denied)?;
            original_receipts.push(held);
        }
        // Retain each ordered program context, rather than exporting only a
        // combined deadline or reauthorizing under the destination publisher.
        for (id, site) in source.controls().iter().chain(source.issued_values()) {
            let descriptor = context
                .source_receipts()
                .iter()
                .find(|d| d.receipt() == *id && d.site() == site)
                .ok_or_else(denied)?;
            let held = Inputs::prepare_receipt_source(conn, descriptor, key).await?;
            fence = fence.intersect(held.accepted_fence());
            let (held_bytes, held_nodes) =
                Inputs::prepared_source_graph_bounds(conn, &[&held], &[], key).await?;
            bytes = bytes.checked_add(held_bytes).ok_or_else(denied)?;
            nodes = nodes.checked_add(held_nodes).ok_or_else(denied)?;
            program.push(held);
        }
        let fields = fields.into_values().map(|(_, row)| row).collect::<Vec<_>>();
        let closure = SecretProvenanceRow {
            entity_type: "wait_completion_metadata".into(),
            entity_id: metadata.snapshot(),
            json_path: String::new(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance {
                origins: closure_origins,
                templates: vec![],
            })?),
        };
        let mut parents = vec![original.frozen_parent()];
        for row in fields.iter().chain(std::iter::once(&closure)) {
            let parent = RuntimeReadParent {
                entity_type: row.entity_type.clone(),
                entity_id: row.entity_id,
                path: row.json_path.clone(),
                provenance: serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?)?,
            };
            if let Some(saved) = parents.iter().find(|saved| {
                saved.entity_type == parent.entity_type
                    && saved.entity_id == parent.entity_id
                    && saved.path == parent.path
            }) {
                if saved.provenance != parent.provenance {
                    return Err(denied());
                }
            } else {
                parents.push(parent);
            }
        }
        nodes = nodes
            .checked_add(fields.len() + receipts.len() + parents.len())
            .ok_or_else(denied)?;
        bytes = bytes
            .checked_add(serde_json::to_vec(&parents)?.len())
            .ok_or_else(denied)?;
        let deepest = original.ceiling_depth;
        if nodes > 256 || bytes > 1024 * 1024 || depth.saturating_add(deepest) >= 32 {
            return Err(denied());
        }
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        let saved = PreparedWaitCompletionSource {
            metadata: metadata.clone(),
            evidence,
            root,
            original,
            fields,
            closure,
            parents,
            receipts,
            original_receipts,
            program,
            fence,
            transaction,
            nodes,
            bytes,
            depth: deepest,
        };
        saved.ensure_fresh()?;
        self.wait_completions
            .insert(metadata.snapshot(), Arc::new(saved));
        Ok((nodes, bytes))
    }
}
fn same_row(a: &SecretProvenanceRow, b: &SecretProvenanceRow) -> bool {
    a.entity_type == b.entity_type
        && a.entity_id == b.entity_id
        && a.json_path == b.json_path
        && a.source_kind == b.source_kind
        && a.source_ref == b.source_ref
}

impl NativeWaitSources {
    pub fn wait_completion_receipt(
        &self,
        metadata: &[NativeSnapshotMetadata],
        ancestry: &BTreeSet<(String, Id, String)>,
        descriptor: &ReceiptSourceDescriptor,
    ) -> Result<Option<Arc<PreparedWaitCompletionSource>>> {
        let mut found: Option<Arc<PreparedWaitCompletionSource>> = None;
        for source in self.wait_completions.values() {
            source.ensure_fresh()?;
            if !metadata.contains(source.metadata()) && !ancestry.contains(&source.scope_node()) {
                continue;
            }
            if !source.covers_receipt(descriptor) {
                continue;
            }
            if found
                .as_ref()
                .is_some_and(|saved| saved.metadata() != source.metadata())
            {
                return Err(denied());
            }
            found = Some(Arc::clone(source));
        }
        Ok(found)
    }
}
