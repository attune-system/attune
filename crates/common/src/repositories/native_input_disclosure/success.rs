//! Ordinary success/warning has its own authenticated recipe and leaf contexts.
use super::super::{
    execution_secret_value::SecretProvenanceRow,
    immutable_input_origin::ImmutableInputOriginRepository,
    inherited_input::{InputSite, ReceiptSourceDescriptor},
    runtime_authority::{ApprovalEffectFence, RuntimeAuthorityRepository},
    runtime_read::RuntimeReadOwner,
};
use super::*;
use crate::{runtime_authority::BoundRuntimeTarget, secret_values::StoredSecretValue};

/// No public data constructor. A's distinct normalized release authenticates
/// the evidence; this token retains ceilings, never current-reader grants.
pub struct PreparedSuccessSource {
    metadata: NativeSnapshotMetadata,
    evidence: NativeSourceEvidence,
    targets: Vec<FiniteExecutionSource>,
    fields: Vec<SecretProvenanceRow>,
    roots: Vec<SecretOrigin>,
    closure: SecretProvenanceRow,
    parents: Vec<RuntimeReadParent>,
    receipts: Vec<(Id, InputSite)>,
    leaves: Vec<ReceiptContext>,
    fence: ApprovalEffectFence,
    transaction: String,
    nodes: usize,
    bytes: usize,
    depth: usize,
}
impl PreparedSuccessSource {
    pub fn metadata(&self) -> &NativeSnapshotMetadata {
        &self.metadata
    }
    pub fn targets(&self) -> &[FiniteExecutionSource] {
        &self.targets
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
            "ordinary_success_metadata".into(),
            self.metadata.snapshot(),
            String::new(),
        )
    }
    fn is_root(&self, origin: &SecretOrigin) -> bool {
        matches!(origin,SecretOrigin::Entity{entity_type,entity_id,path} if entity_type=="ordinary_success_metadata"&&*entity_id==self.metadata.snapshot()&&path.is_empty())
    }
    fn is_field(&self, origin: &SecretOrigin) -> bool {
        self.roots.contains(origin)
            || matches!(origin,SecretOrigin::Entity{entity_type,entity_id,path} if self.fields.iter().any(|row|row.entity_type==*entity_type&&row.entity_id==*entity_id&&row.json_path==*path))
    }
    pub(crate) fn covers_receipt(&self, source: &ReceiptSourceDescriptor) -> bool {
        self.receipts
            .iter()
            .any(|(id, site)| *id == source.receipt() && site == source.site())
    }
    pub fn row_site(&self, row: &SecretProvenanceRow) -> Result<Option<InputSite>> {
        if same_row(row, &self.closure) {
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
        if self.roots.contains(origin) {
            let rows = self
                .fields
                .iter()
                .filter(|row| row.entity_type == *entity_type && row.entity_id == *entity_id)
                .cloned()
                .collect::<Vec<_>>();
            return Ok(rows);
        }
        let row = self
            .fields
            .iter()
            .find(|row| {
                row.entity_type == *entity_type
                    && row.entity_id == *entity_id
                    && row.json_path == *path
            })
            .ok_or_else(denied)?;
        Ok(vec![row.clone()])
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        for leaf in &self.leaves {
            leaf.ensure_fresh()?;
        }
        Ok(())
    }
    pub(crate) async fn ensure_held_on(&self, conn: &mut PgConnection) -> Result<()> {
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if transaction != self.transaction {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await?;
        for leaf in &self.leaves {
            leaf.ensure_fresh_on(conn).await?;
        }
        self.ensure_fresh()
    }
    pub fn output_leaf_count(&self) -> usize {
        self.evidence
            .success_metadata()
            .map_or(0, |source| source.output_leaves().len())
    }
}
impl NativeWaitSources {
    pub fn success(
        &self,
        metadata: &[NativeSnapshotMetadata],
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<PreparedSuccessSource>>> {
        let mut found: Option<Arc<PreparedSuccessSource>> = None;
        for item in metadata
            .iter()
            .filter(|item| item.source_kind() == "ordinary_success_metadata")
        {
            let saved = self.successes.get(&item.snapshot()).ok_or_else(denied)?;
            saved.ensure_fresh()?;
            if saved.metadata() != item {
                return Err(denied());
            }
            if !saved.is_root(origin) && !saved.is_field(origin) {
                continue;
            }
            if found
                .as_ref()
                .is_some_and(|source| source.metadata() != item)
            {
                return Err(denied());
            }
            found = Some(Arc::clone(saved));
        }
        Ok(found)
    }
    pub fn success_in_scope(
        &self,
        ancestry: &BTreeSet<(String, Id, String)>,
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<PreparedSuccessSource>>> {
        let mut found: Option<Arc<PreparedSuccessSource>> = None;
        for source in self
            .successes
            .values()
            .filter(|source| ancestry.contains(&source.scope_node()))
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
    pub(super) async fn prepare_success(
        &mut self,
        conn: &mut PgConnection,
        context: &ReceiptContext,
        metadata: &NativeSnapshotMetadata,
        key: &str,
        depth: usize,
    ) -> Result<(usize, usize)> {
        if let Some(saved) = self.successes.get(&metadata.snapshot()) {
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
        let evidence = NativeInputSnapshotRepository::source_evidence(conn, metadata, key).await?;
        let storage =
            NativeInputSnapshotRepository::storage_evidence(conn, metadata.snapshot()).await?;
        let source = evidence.success_metadata().ok_or_else(denied)?;
        let stored = storage.success_metadata().ok_or_else(denied)?;
        let root = SecretOrigin::Entity {
            entity_type: "ordinary_success_metadata".into(),
            entity_id: metadata.snapshot(),
            path: String::new(),
        };
        if metadata.source_kind() != "ordinary_success_metadata"
            || source.snapshot() != metadata.snapshot()
            || stored.snapshot() != metadata.snapshot()
            || source.transaction() != stored.transaction()
            || source.execution() != stored.execution()
            || source.completed_execution() != stored.completed_execution()
            || metadata.source_site() != Some(("execution_result", source.execution(), ""))
            || metadata.result_pointer_mappings().is_some()
            || metadata.result_contract_kind().is_some()
            || metadata.generation().is_some()
            || metadata.target() != source.target()
            || metadata.read_owner() != source.read_owner()
            || evidence.origins() != [root.clone()]
            || !context.source_origins().contains(&root)
            || !evidence.fields().is_empty()
            || !storage.archive_fields().is_empty()
            || !evidence.completion_sources().is_empty()
            || storage.wait_release().is_some()
            || storage.result_view_release().is_some()
            || storage.completion_release().is_some()
            || !storage.completion_sources().is_empty()
            || serde_json::to_value(evidence.contract())?
                != serde_json::to_value(source.original_contract().output.as_ref())?
            || serde_json::to_value(storage.contract())?
                != serde_json::to_value(source.original_contract().output.as_ref())?
            || serde_json::to_value(evidence.contributor_manifests())?
                != serde_json::to_value(source.root_contributor_manifests())?
            || serde_json::to_value(storage.contributor_manifests())?
                != serde_json::to_value(source.root_contributor_manifests())?
            || evidence.renderer_admission() != source.original_renderer_admission()
        {
            return Err(denied());
        }
        source.original_contract().validate()?;
        let (parent, parent_fence) = execution_target(
            conn,
            source.target(),
            source.read_owner(),
            source.original_renderer_admission(),
        )
        .await?;
        let mut targets = vec![parent];
        let mut fence = context.accepted_fence().intersect(&parent_fence);
        let mut fields = BTreeMap::new();
        let mut root_origins = Vec::new();
        let mut roots = Vec::new();
        let mut nodes = 1usize;
        let mut bytes = evidence.retained_bytes();
        let mut deepest = 0usize;
        for (kind, values) in [
            ("execution_config", source.original_config_fields()),
            ("execution_result", source.original_result_fields()),
            ("workflow_variables", source.original_variable_fields()),
        ] {
            for field in values {
                collect_field(
                    &mut fields,
                    kind,
                    source.execution(),
                    &field,
                    &mut root_origins,
                )?;
            }
        }
        let root_authorities =
            manifest_authorities(conn, source.root_contributor_manifests()).await?;
        if !root_origins.is_empty() {
            if root_authorities.is_empty() {
                return Err(denied());
            }
            let charge = ImmutableInputOriginRepository::require_counted(
                conn,
                &root_authorities,
                &root_origins,
            )
            .await?;
            nodes = nodes.checked_add(charge.0).ok_or_else(denied)?;
            bytes = bytes.checked_add(charge.1).ok_or_else(denied)?;
            deepest = deepest.max(charge.2);
        }
        let children = source.children();
        let mut seen = BTreeSet::new();
        if source.original_parent().id != source.execution()
            || source.workflow().execution != source.execution()
        {
            return Err(denied());
        }
        if children.is_empty() || source.original_child_contracts().len() != children.len() {
            return Err(denied());
        }
        for child in children {
            if !seen.insert(child.execution())
                || child.target().id != child.execution()
                || child.status() != "completed"
                || child.timed_out()
                || child
                    .receipt_edges()
                    .iter()
                    .any(|edge| !source.receipt_edges().contains(edge))
            {
                return Err(denied());
            }
            let (_, contract) = source
                .original_child_contracts()
                .iter()
                .find(|(id, _)| *id == child.execution())
                .ok_or_else(denied)?;
            contract.validate()?;
            let (target, child_fence, charge) = prepare_execution(
                conn,
                child.target(),
                child.read_owner(),
                child.admission(),
                child.contributor_manifests(),
                child.origins(),
            )
            .await?;
            roots.push(target.root_origin().clone());
            let mut ordinary = Vec::new();
            for field in child.original_fields() {
                collect_field(
                    &mut fields,
                    "execution_result",
                    child.execution(),
                    &field,
                    &mut ordinary,
                )?;
            }
            if ordinary
                .iter()
                .any(|origin| !child.origins().contains(origin))
            {
                return Err(denied());
            }
            fence = fence.intersect(&child_fence);
            targets.push(target);
            nodes = nodes
                .checked_add(charge.0 + child.original_fields().len() + child.receipt_edges().len())
                .ok_or_else(denied)?;
            bytes = bytes.checked_add(charge.1).ok_or_else(denied)?;
            deepest = deepest.max(charge.2);
        }
        if !seen.contains(&source.completed_execution()) {
            return Err(denied());
        }
        // Every archived field is walked at its own site. Never flatten child
        // schema-local origins into a site-less aggregate provenance row.
        let mut closure_origins = Vec::new();
        let mut leaves = Vec::new();
        let mut leaf_rows = Vec::new();
        let receipts = source.receipt_edges().to_vec();
        if source
            .controls()
            .iter()
            .any(|edge| !receipts.contains(edge))
        {
            return Err(denied());
        }
        for (id, site) in &receipts {
            let descriptor = context
                .source_receipts()
                .iter()
                .find(|descriptor| descriptor.receipt() == *id && descriptor.site() == site)
                .ok_or_else(denied)?;
            let origin = descriptor.origin();
            if !closure_origins.contains(&origin) {
                closure_origins.push(origin);
            }
        }
        let mut leaf_paths = BTreeSet::new();
        for leaf in source.output_leaves() {
            if !leaf_paths.insert(leaf.path()) {
                return Err(denied());
            }
            if leaf.encryption_key_hash() != crate::crypto::hash_encryption_key(key)
                || leaf.proof_digest().is_empty()
            {
                return Err(denied());
            }
            let descriptor = context
                .source_receipts()
                .iter()
                .find(|descriptor| {
                    descriptor.receipt() == leaf.receipt()
                        && descriptor.site().execution == source.execution()
                        && descriptor.site().kind
                            == super::super::inherited_input::InputEntityKind::ExecutionResult
                        && descriptor.site().purpose
                            == super::super::inherited_input::InputSitePurpose::Value
                        && descriptor.site().path == leaf.path()
                })
                .ok_or_else(denied)?;
            let held =
                super::super::inherited_input::InheritedInputRepository::prepare_receipt_source(
                    conn, descriptor, key,
                )
                .await?;
            fence = fence.intersect(held.accepted_fence());
            let classification =
                super::super::immutable_input_origin::ReceiptDisclosureLeaves::from_context(&held)?;
            // The archive evidence accounts for the recipe and metadata. The
            // reopened leaf additionally retains plaintext and receipt edges.
            let held_bytes = serde_json::to_vec(held.accepted_value())?
                .len()
                .checked_add(serde_json::to_vec(&held.provenance())?.len())
                .and_then(|n| n.checked_add(classification.source_descriptor_bytes().ok()?))
                .ok_or_else(denied)?;
            bytes = bytes
                .checked_add(held_bytes)
                .and_then(|n| n.checked_add(serde_json::to_vec(leaf.original_proof()).ok()?.len()))
                .and_then(|n| n.checked_add(serde_json::to_vec(leaf.encrypted_value()).ok()?.len()))
                .ok_or_else(denied)?;
            let row = SecretProvenanceRow {
                entity_type: "execution_result".into(),
                entity_id: source.execution(),
                json_path: leaf.path().into(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&held.provenance())?),
            };
            let slot = (
                row.entity_type.clone(),
                row.entity_id,
                row.json_path.clone(),
            );
            if fields.contains_key(&slot) {
                return Err(denied());
            }
            // Actual output leaf receipts, not a fabricated authored succeeded field.
            leaves.push(held);
            leaf_rows.push(row);
        }
        let mut fields = fields.into_values().map(|(_, row)| row).collect::<Vec<_>>();
        fields.extend(leaf_rows);
        let closure = SecretProvenanceRow {
            entity_type: "ordinary_success_metadata".into(),
            entity_id: metadata.snapshot(),
            json_path: String::new(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance {
                origins: closure_origins,
                templates: vec![],
            })?),
        };
        let mut parents = Vec::new();
        for row in fields.iter().chain(std::iter::once(&closure)) {
            let mut provenance: SecretProvenance =
                serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?)?;
            if provenance.origins.is_empty() {
                provenance = structural_terminal();
            }
            parents.push(RuntimeReadParent {
                entity_type: row.entity_type.clone(),
                entity_id: row.entity_id,
                path: row.json_path.clone(),
                provenance,
            });
        }
        let original_receipt_parents =
            super::super::immutable_input_origin::freeze_receipt_context(context)?;
        for root in &roots {
            let SecretOrigin::Entity {
                entity_type,
                entity_id,
                path,
            } = root
            else {
                return Err(denied());
            };
            if !parents.iter().any(|parent| {
                parent.entity_type == *entity_type
                    && parent.entity_id == *entity_id
                    && parent.path == *path
            }) {
                parents.push(RuntimeReadParent {
                    entity_type: entity_type.clone(),
                    entity_id: *entity_id,
                    path: path.clone(),
                    // A real condition may already classify this root as an
                    // outcome observation. Preserve that authenticated terminal
                    // rather than relabeling it as a public VALUE source.
                    provenance: original_receipt_parents
                        .iter()
                        .find(|parent| {
                            parent.entity_type == *entity_type
                                && parent.entity_id == *entity_id
                                && parent.path == *path
                        })
                        .map(|parent| parent.provenance.clone())
                        .unwrap_or_else(structural_terminal),
                });
            }
        }
        nodes = nodes
            .checked_add(fields.len() + leaves.len() + receipts.len() + parents.len())
            .ok_or_else(denied)?;
        bytes = bytes
            .checked_add(serde_json::to_vec(&parents)?.len())
            .ok_or_else(denied)?;
        if nodes > 256 || bytes > 1024 * 1024 || depth.saturating_add(deepest) >= 32 {
            return Err(denied());
        }
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        let saved = PreparedSuccessSource {
            metadata: metadata.clone(),
            evidence,
            targets,
            fields,
            roots,
            closure,
            parents,
            receipts,
            leaves,
            fence,
            transaction,
            nodes,
            bytes,
            depth: deepest,
        };
        saved.ensure_fresh()?;
        self.successes.insert(metadata.snapshot(), Arc::new(saved));
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

fn structural_terminal() -> SecretProvenance {
    // Same frozen public-source terminal as PreparedWaitSource::frozen_parent.
    // It records a structurally complete parent, never plaintext authority.
    // Replay must first recover this exact success through its genuine receipt;
    // the ordinary Local-origin evaluator continues to reject this marker.
    SecretProvenance {
        origins: vec![SecretOrigin::Local {
            source_kind: "receipt_public_source".into(),
            source_ref: None,
        }],
        templates: vec![],
    }
}

pub struct FiniteExecutionSource {
    pub(crate) target: BoundRuntimeTarget,
    pub(crate) owner: RuntimeReadOwner,
    pub(crate) release: Id,
    pub(crate) release_digest: String,
    pub(crate) action_ref: String,
    root: SecretOrigin,
    payload: SecretProvenance,
}
impl FiniteExecutionSource {
    pub(crate) fn root_origin(&self) -> &SecretOrigin {
        &self.root
    }
}
pub(super) async fn execution_target(
    conn: &mut PgConnection,
    target: &BoundRuntimeTarget,
    owner: &RuntimeReadOwner,
    admission: Id,
) -> Result<(FiniteExecutionSource, ApprovalEffectFence)> {
    let record = RuntimeAuthorityRepository::require_retained_admission(conn, admission).await?;
    if target.kind != "execution"
        || target.id <= 0
        || record.owner_kind != "execution"
        || record.owner_id != target.id
    {
        return Err(denied());
    }
    owner.execution_reference(target, record.authority.binding.action_ref())?;
    let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, admission).await?;
    Ok((
        FiniteExecutionSource {
            target: target.clone(),
            owner: owner.clone(),
            release: record.authority.target_release_id,
            release_digest: record.authority.target_release_digest.clone(),
            action_ref: record.authority.binding.action_ref().to_owned(),
            root: SecretOrigin::Entity {
                entity_type: "execution_result".into(),
                entity_id: target.id,
                path: String::new(),
            },
            payload: SecretProvenance::default(),
        },
        fence,
    ))
}
pub(super) async fn prepare_execution(
    conn: &mut PgConnection,
    target: &BoundRuntimeTarget,
    owner: &RuntimeReadOwner,
    admission: Id,
    manifests: &[super::super::accepted_input::AcceptedContributorManifest],
    origins: &[SecretOrigin],
) -> Result<(
    FiniteExecutionSource,
    ApprovalEffectFence,
    (usize, usize, usize),
)> {
    let (mut saved, fence) = execution_target(conn, target, owner, admission).await?;
    if !origins.contains(&saved.root) {
        return Err(denied());
    }
    saved.payload.origins = origins
        .iter()
        .filter(|origin| **origin != saved.root)
        .cloned()
        .collect();
    let ordinary = saved
        .payload
        .origins
        .iter()
        .filter(|origin| {
            !matches!(origin, SecretOrigin::Local { .. })
                && !super::super::immutable_input_origin::is_input_receipt(origin)
        })
        .cloned()
        .collect::<Vec<_>>();
    let authorities = manifest_authorities(conn, manifests).await?;
    let charges = if ordinary.is_empty() {
        (0, 0, 0)
    } else {
        if authorities.is_empty() {
            return Err(denied());
        }
        super::super::immutable_input_origin::ImmutableInputOriginRepository::require_counted(
            conn,
            &authorities,
            &ordinary,
        )
        .await?
    };
    // Each source's original contributors are evaluated separately above. The
    // flattened completion manifest accessor is intentionally never consumed.
    Ok((
        saved,
        fence,
        (charges.0 + 1 + manifests.len(), charges.1, charges.2),
    ))
}
pub(super) fn collect_field(
    rows: &mut BTreeMap<(String, Id, String), (StoredSecretValue, SecretProvenanceRow)>,
    kind: &str,
    id: Id,
    field: &StoredSecretValue,
    ordinary: &mut Vec<SecretOrigin>,
) -> Result<()> {
    let provenance = match field.source_kind.as_str() {
        "provenance" => serde_json::from_str::<SecretProvenance>(
            field.source_ref.as_deref().ok_or_else(denied)?,
        )?,
        "parameter_schema" => crate::secret_values::SecretSource::ParameterSchema {
            path: field.source_ref.clone().ok_or_else(denied)?,
        }
        .provenance(),
        _ => return Err(denied()),
    };
    if provenance.origins.is_empty() {
        return Err(denied());
    }
    for origin in &provenance.origins {
        if !matches!(origin, SecretOrigin::Local { .. })
            && !super::super::immutable_input_origin::is_input_receipt(origin)
            && !ordinary.contains(origin)
        {
            ordinary.push(origin.clone());
        }
    }
    let site = (kind.to_owned(), id, field.json_path.clone());
    if let Some((saved, _)) = rows.get(&site) {
        if saved != field {
            return Err(denied());
        }
    } else {
        rows.insert(
            site,
            (
                field.clone(),
                SecretProvenanceRow {
                    entity_type: kind.into(),
                    entity_id: id,
                    json_path: field.json_path.clone(),
                    source_kind: "provenance".into(),
                    source_ref: Some(serde_json::to_string(&provenance)?),
                },
            ),
        );
    }
    Ok(())
}

impl NativeWaitSources {
    pub(crate) fn success_receipt(
        &self,
        metadata: &[NativeSnapshotMetadata],
        ancestry: &BTreeSet<(String, Id, String)>,
        descriptor: &ReceiptSourceDescriptor,
    ) -> Result<Option<Arc<PreparedSuccessSource>>> {
        let mut found: Option<Arc<PreparedSuccessSource>> = None;
        for source in self.successes.values() {
            if !metadata.contains(source.metadata()) && !ancestry.contains(&source.scope_node()) {
                continue;
            }
            source.ensure_fresh()?;
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
