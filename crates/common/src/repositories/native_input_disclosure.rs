//! Source-phase native evidence for the canonical C disclosure frontier.
//! Opaque A contexts issue this evidence; it is never a reader permission.
use super::{
    inherited_input::ReceiptContext,
    native_input_snapshot::{
        NativeInputSnapshotRepository, NativeSnapshotMetadata, NativeSourceEvidence,
    },
    runtime_read::RuntimeReadParent,
};
use crate::{
    delegation::DelegationAuthority,
    models::Id,
    rbac::{Action, Grant, Resource},
    secret_provenance::{SecretOrigin, SecretProvenance},
    Error, Result,
};
use sqlx::PgConnection;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

mod success;
pub use success::PreparedSuccessSource;

fn denied() -> Error {
    Error::PermissionDenied("Native input disclosure evidence is unavailable".into())
}

/// No serde/data constructor. Kept on the same frontier as the retaining A
/// context and its release/source/approval fences, including final deadlines.
pub struct PreparedWaitSource {
    metadata: NativeSnapshotMetadata,
    evidence: NativeSourceEvidence,
    payload: SecretProvenance,
    source_admission: Id,
    source_execution: Id,
    release: Id,
    release_digest: String,
    action_ref: String,
    ceiling_nodes: usize,
    ceiling_bytes: usize,
    ceiling_depth: usize,
    transaction: String,
    fence: super::runtime_authority::ApprovalEffectFence,
}

impl PreparedWaitSource {
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }
    pub(crate) async fn ensure_held_on(&self, conn: &mut PgConnection) -> Result<()> {
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if transaction != self.transaction {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await
    }
    pub fn metadata(&self) -> &NativeSnapshotMetadata {
        &self.metadata
    }
    pub fn source_admission(&self) -> Id {
        self.source_admission
    }
    pub(crate) fn source_execution(&self) -> Id {
        self.source_execution
    }
    pub(crate) fn release(&self) -> Id {
        self.release
    }
    pub(crate) fn release_digest(&self) -> &str {
        &self.release_digest
    }
    pub(crate) fn action_ref(&self) -> &str {
        &self.action_ref
    }
    pub fn contract(&self) -> Option<&crate::value_schema::Contract> {
        self.evidence.contract()
    }
    pub fn root(&self) -> SecretOrigin {
        SecretOrigin::Entity {
            entity_type: self.metadata.source_kind().into(),
            entity_id: self.metadata.target().id,
            path: String::new(),
        }
    }
    pub fn retained_bytes(&self) -> usize {
        self.evidence.retained_bytes() + self.ceiling_bytes
    }
    pub fn retained_nodes(&self) -> usize {
        1 + self.evidence.fields().len()
            + self.evidence.contributor_manifests().len()
            + self.ceiling_nodes
    }
    pub fn frozen_parent(&self) -> RuntimeReadParent {
        let provenance = if self.payload.origins.is_empty() {
            // Structural terminal only. It cannot authorize a raw Local origin;
            // replay must recover this exact source through an A context first.
            SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: "receipt_public_source".into(),
                    source_ref: None,
                }],
                templates: vec![],
            }
        } else {
            self.payload.clone()
        };
        RuntimeReadParent {
            entity_type: self.metadata.source_kind().into(),
            entity_id: self.metadata.target().id,
            path: String::new(),
            provenance,
        }
    }
}

#[derive(Default)]
pub struct NativeWaitSources {
    executions: BTreeMap<Id, Arc<PreparedWaitSource>>,
    caches: BTreeMap<Id, Arc<PreparedCacheSource>>,
    successes: BTreeMap<Id, Arc<PreparedSuccessSource>>,
}

/// Authenticated original cache page and its original task freshness policy.
/// A policy with no deadline is not equivalent to require_fresh=false.
pub struct PreparedCacheSource {
    metadata: NativeSnapshotMetadata,
    evidence: NativeSourceEvidence,
    transaction: String,
    fence: super::runtime_authority::ApprovalEffectFence,
}

impl PreparedCacheSource {
    pub fn metadata(&self) -> &NativeSnapshotMetadata {
        &self.metadata
    }
    pub fn root(&self) -> &SecretOrigin {
        &self.evidence.origins()[0]
    }
    pub fn retained_bytes(&self) -> usize {
        self.evidence.retained_bytes()
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        self.evidence
            .cache_read_policy()
            .ok_or_else(denied)?
            .ensure_fresh()
    }
    pub(crate) async fn ensure_held_on(&self, conn: &mut PgConnection) -> Result<()> {
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if transaction != self.transaction {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await?;
        self.ensure_fresh()
    }
    pub fn frozen_parent(&self) -> RuntimeReadParent {
        let SecretOrigin::Entity {
            entity_type,
            entity_id,
            path,
        } = self.root()
        else {
            unreachable!()
        };
        RuntimeReadParent {
            entity_type: entity_type.clone(),
            entity_id: *entity_id,
            path: path.clone(),
            provenance: SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: "receipt_public_source".into(),
                    source_ref: None,
                }],
                templates: vec![],
            },
        }
    }
}

impl NativeWaitSources {
    fn source_count(&self) -> usize {
        self.executions.len() + self.caches.len() + self.successes.len()
    }

    /// Called only before capture/consumer target locks. Returns new graph
    /// charges so the caller uses its existing node/byte budget, not a reset.
    pub async fn prepare_context(
        &mut self,
        conn: &mut PgConnection,
        context: &ReceiptContext,
        key: &str,
        depth: usize,
    ) -> Result<(usize, usize)> {
        context.ensure_fresh_on(conn).await?;
        let mut nodes = 0usize;
        let mut bytes = 0usize;
        for metadata in context.native_source_metadata() {
            if metadata.source_kind() == "ordinary_success_metadata" {
                let charges = self
                    .prepare_success(conn, context, metadata, key, depth)
                    .await?;
                nodes = nodes.checked_add(charges.0).ok_or_else(denied)?;
                bytes = bytes.checked_add(charges.1).ok_or_else(denied)?;
                continue;
            }
            if metadata.source_kind() == "cache" {
                if let Some(saved) = self.caches.get(&metadata.snapshot()) {
                    if saved.metadata() != metadata {
                        return Err(denied());
                    }
                    saved.ensure_held_on(conn).await?;
                    continue;
                }
                if self.source_count() >= 256 {
                    return Err(denied());
                }
                lock_snapshot_archive(conn, metadata.snapshot()).await?;
                let evidence =
                    NativeInputSnapshotRepository::source_evidence(conn, metadata, key).await?;
                let target = metadata.target();
                if target.kind != "cache_namespace"
                    || target.id <= 0
                    || metadata
                        .generation()
                        .is_none_or(|generation| generation <= 0)
                    || metadata.source_site().is_some()
                    || evidence.origins().len() != 1
                    || !matches!(&evidence.origins()[0], SecretOrigin::Entity { entity_type, entity_id, .. } if entity_type == "cache_namespace" && *entity_id == target.id)
                    || !context.source_origins().contains(&evidence.origins()[0])
                    || evidence.cache_read_policy().is_none()
                    || !evidence.fields().is_empty()
                    || !evidence.contributor_manifests().is_empty()
                    || !evidence.source_admissions().is_empty()
                {
                    return Err(denied());
                }
                let prepared = PreparedCacheSource {
                    metadata: metadata.clone(),
                    evidence,
                    transaction: sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
                        .fetch_one(&mut *conn)
                        .await?,
                    fence: context.accepted_fence().clone(),
                };
                prepared.ensure_fresh()?;
                nodes = nodes.checked_add(1).ok_or_else(denied)?;
                bytes = bytes
                    .checked_add(prepared.retained_bytes())
                    .ok_or_else(denied)?;
                if nodes > 256 || bytes > 1024 * 1024 {
                    return Err(denied());
                }
                self.caches.insert(metadata.snapshot(), Arc::new(prepared));
                continue;
            }
            if !matches!(
                metadata.source_kind(),
                "execution_result" | "inquiry_response" | "queue_item_result"
            ) {
                continue;
            }
            if let Some(saved) = self.executions.get(&metadata.snapshot()) {
                if saved.metadata() != metadata || depth.saturating_add(saved.ceiling_depth) >= 32 {
                    return Err(denied());
                }
                continue;
            }
            if self.source_count() >= 256 {
                return Err(denied());
            }
            lock_execution_wait_snapshot(conn, metadata.snapshot()).await?;
            let evidence =
                NativeInputSnapshotRepository::source_evidence(conn, metadata, key).await?;
            let prepared = prepare_wait_source(conn, metadata, evidence, context).await?;
            if depth.saturating_add(prepared.ceiling_depth) >= 32 {
                return Err(denied());
            }
            // These are the same direct origins A put on the retaining receipt.
            // The canonical queue, not a second walker, will authorize all of
            // them for the current reader, including original typed receipts.
            if prepared
                .evidence
                .origins()
                .iter()
                .any(|origin| !context.source_origins().contains(origin))
            {
                return Err(denied());
            }
            nodes = nodes
                .checked_add(prepared.retained_nodes())
                .ok_or_else(denied)?;
            bytes = bytes
                .checked_add(prepared.retained_bytes())
                .ok_or_else(denied)?;
            let held_bytes = self.executions.values().try_fold(0usize, |total, saved| {
                total.checked_add(saved.retained_bytes()).ok_or_else(denied)
            })?;
            let held_nodes = self
                .executions
                .values()
                .map(|saved| saved.retained_nodes())
                .sum::<usize>();
            if nodes > 256
                || bytes > 1024 * 1024
                || held_nodes + prepared.retained_nodes() > 256
                || held_bytes
                    .checked_add(prepared.retained_bytes())
                    .is_none_or(|total| total > 1024 * 1024)
            {
                return Err(denied());
            }
            self.executions
                .insert(metadata.snapshot(), Arc::new(prepared));
        }
        let total_bytes = self
            .executions
            .values()
            .map(|source| source.retained_bytes())
            .chain(self.caches.values().map(|source| source.retained_bytes()))
            .chain(
                self.successes
                    .values()
                    .map(|source| source.retained_bytes()),
            )
            .try_fold(0usize, |total, bytes| {
                total.checked_add(bytes).ok_or_else(denied)
            })?;
        let total_nodes = self
            .executions
            .values()
            .map(|source| source.retained_nodes())
            .chain(
                self.successes
                    .values()
                    .map(|source| source.retained_nodes()),
            )
            .try_fold(self.caches.len(), |total, nodes| {
                total.checked_add(nodes).ok_or_else(denied)
            })?;
        if total_bytes > 1024 * 1024 || total_nodes > 256 {
            return Err(denied());
        }
        Ok((nodes, bytes))
    }

    pub fn ensure_fresh(&self) -> Result<()> {
        for source in self.successes.values() {
            source.ensure_fresh()?;
        }
        for source in self.caches.values() {
            source.ensure_fresh()?;
        }
        for source in self.executions.values() {
            source.ensure_fresh()?;
        }
        Ok(())
    }

    pub async fn ensure_held_on(&self, conn: &mut PgConnection) -> Result<()> {
        for source in self.successes.values() {
            source.ensure_held_on(conn).await?;
        }
        for source in self.caches.values() {
            source.ensure_held_on(conn).await?;
        }
        for source in self.executions.values() {
            source.ensure_held_on(conn).await?;
        }
        Ok(())
    }

    pub fn cache(
        &self,
        metadata: &[NativeSnapshotMetadata],
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<PreparedCacheSource>>> {
        let mut found: Option<Arc<PreparedCacheSource>> = None;
        for item in metadata.iter().filter(|item| item.source_kind() == "cache") {
            let saved = self.caches.get(&item.snapshot()).ok_or_else(denied)?;
            if saved.metadata() != item {
                return Err(denied());
            }
            saved.ensure_fresh()?;
            if saved.root() != origin {
                continue;
            }
            if found
                .as_ref()
                .is_some_and(|previous| previous.metadata() != saved.metadata())
            {
                return Err(denied());
            }
            found = Some(Arc::clone(saved));
        }
        Ok(found)
    }

    pub fn execution(
        &self,
        metadata: &[NativeSnapshotMetadata],
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<PreparedWaitSource>>> {
        self.wait_source(metadata, origin, &["execution_result"])
    }

    pub fn related(
        &self,
        metadata: &[NativeSnapshotMetadata],
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<PreparedWaitSource>>> {
        self.wait_source(metadata, origin, &["inquiry_response", "queue_item_result"])
    }

    fn wait_source(
        &self,
        metadata: &[NativeSnapshotMetadata],
        origin: &SecretOrigin,
        kinds: &[&str],
    ) -> Result<Option<Arc<PreparedWaitSource>>> {
        let mut found: Option<Arc<PreparedWaitSource>> = None;
        for item in metadata {
            if !kinds.contains(&item.source_kind()) {
                continue;
            }
            let source = self.executions.get(&item.snapshot()).ok_or_else(denied)?;
            source.ensure_fresh()?;
            if source.metadata() != item {
                return Err(denied());
            }
            if source.root() != *origin {
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

/// Protect the immutable archive and actual normalized release before A reloads
/// and authenticates them. A raw wait row may already be collected; when present
/// it remains locked through all consumer waits. These are fixed source rows,
/// not a second provenance traversal or an authority constructor.
async fn lock_execution_wait_snapshot(conn: &mut PgConnection, snapshot: Id) -> Result<()> {
    lock_snapshot_archive(conn, snapshot).await?;
    let release: Option<Id> = sqlx::query_scalar(
        "SELECT snapshot FROM runtime_native_wait_release WHERE snapshot=$1 FOR SHARE",
    )
    .bind(snapshot)
    .fetch_optional(&mut *conn)
    .await?;
    if release != Some(snapshot) {
        return Err(denied());
    }
    let _: Option<Id> = sqlx::query_scalar("SELECT w.id FROM workflow_task_wait w JOIN runtime_native_wait_release r ON r.wait=w.id WHERE r.snapshot=$1 FOR SHARE OF w")
        .bind(snapshot).fetch_optional(&mut *conn).await?;
    Ok(())
}

async fn lock_snapshot_archive(conn: &mut PgConnection, snapshot: Id) -> Result<()> {
    let saved: Option<Id> =
        sqlx::query_scalar("SELECT id FROM runtime_native_input_snapshot WHERE id=$1 FOR SHARE")
            .bind(snapshot)
            .fetch_optional(&mut *conn)
            .await?;
    if saved != Some(snapshot) {
        return Err(denied());
    }
    let fields: Vec<Id> = sqlx::query_scalar("SELECT id FROM execution_secret_value WHERE entity_type='native_input_snapshot' AND entity_id=$1 ORDER BY id LIMIT 257 FOR SHARE")
        .bind(snapshot).fetch_all(&mut *conn).await?;
    if fields.len() > 256 {
        return Err(denied());
    }
    Ok(())
}

// This is the canonical captured-human/queue authority body, consumed only from
// A's authenticated manifest. It has no public constructor or serde proof API.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedInputAuthority {
    identity_id: Id,
    attributes: HashMap<String, serde_json::Value>,
    grants: Vec<Grant>,
}

async fn original_authorities(
    conn: &mut PgConnection,
    evidence: &NativeSourceEvidence,
) -> Result<Vec<DelegationAuthority>> {
    manifest_authorities(conn, evidence.contributor_manifests()).await
}

async fn manifest_authorities(
    conn: &mut PgConnection,
    manifests: &[super::accepted_input::AcceptedContributorManifest],
) -> Result<Vec<DelegationAuthority>> {
    if manifests.len() > 256 {
        return Err(denied());
    }
    let queue_count = manifests
        .iter()
        .filter(|item| item.kind() == "queue")
        .count();
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for manifest in manifests {
        if !seen.insert((manifest.kind(), manifest.item(), manifest.name()))
            || manifest.body().is_null()
        {
            return Err(denied());
        }
        match manifest.kind() {
            "execution" | "queue" => {
                if manifest.kind() == "queue" {
                    if manifest.item() <= 0
                        || usize::try_from(manifest.expected()).ok() != Some(queue_count)
                    {
                        return Err(denied());
                    }
                } else if manifest.item() != 0
                    || !manifest.name().is_empty()
                    || manifest.expected() != 1
                {
                    return Err(denied());
                }
                let captured: CapturedInputAuthority =
                    serde_json::from_value(manifest.body().clone()).map_err(|_| denied())?;
                if captured.identity_id <= 0
                    || captured.grants.iter().any(|grant| {
                        !matches!(
                            grant.resource,
                            Resource::Keys
                                | Resource::Packs
                                | Resource::Executions
                                | Resource::Artifacts
                                | Resource::QueueItems
                                | Resource::Queues
                                | Resource::Events
                                | Resource::Triggers
                                | Resource::Inquiries
                        ) || grant
                            .actions
                            .iter()
                            .any(|action| !matches!(action, Action::Read | Action::Decrypt))
                    })
                {
                    return Err(denied());
                }
                result.push(DelegationAuthority::new(
                    captured.identity_id,
                    captured.attributes,
                    captured.grants,
                ));
            }
            "rule" => {
                if manifest.item() != 0 || !manifest.name().is_empty() || manifest.expected() != 1 {
                    return Err(denied());
                }
                let captured: super::rule_authority::RuleInputAuthority =
                    serde_json::from_value(manifest.body().clone()).map_err(|_| denied())?;
                result.push(captured.input_authority(conn).await?);
            }
            _ => return Err(denied()),
        }
    }
    Ok(result)
}

async fn prepare_wait_source(
    conn: &mut PgConnection,
    metadata: &NativeSnapshotMetadata,
    evidence: NativeSourceEvidence,
    context: &ReceiptContext,
) -> Result<PreparedWaitSource> {
    let target = metadata.target();
    let target_kind = match metadata.source_kind() {
        "execution_result" => "execution",
        "inquiry_response" => "inquiry",
        "queue_item_result" => "work_queue_item",
        _ => return Err(denied()),
    };
    if target.kind != target_kind
        || target.id <= 0
        || metadata.generation().is_some()
        || metadata.source_site() != Some((metadata.source_kind(), target.id, ""))
        || evidence.source_admissions().len() != 1
        || target_kind == "inquiry" && evidence.contract().is_none()
        || target_kind == "work_queue_item" && evidence.contract().is_some()
    {
        return Err(denied());
    }
    let source_admission = evidence.source_admissions()[0];
    let record = super::runtime_authority::RuntimeAuthorityRepository::require_retained_admission(
        conn,
        source_admission,
    )
    .await?;
    if record.owner_kind != "execution"
        || record.owner_id <= 0
        || target_kind == "execution" && record.owner_id != target.id
    {
        return Err(denied());
    }
    if let Some(contract) = evidence.contract() {
        crate::value_schema::CompiledContract::new(contract).map_err(|_| denied())?;
        if target_kind == "execution"
            && contract.source.component_ref() != record.authority.binding.action_ref()
        {
            return Err(denied());
        }
    }
    let root = SecretOrigin::Entity {
        entity_type: metadata.source_kind().into(),
        entity_id: target.id,
        path: String::new(),
    };
    let mut expected = vec![root];
    let mut payload = SecretProvenance::default();
    let mut ordinary = Vec::new();
    for field in evidence.fields() {
        let provenance = if field.source_kind == "provenance" {
            serde_json::from_str::<SecretProvenance>(
                field.source_ref.as_deref().ok_or_else(denied)?,
            )
            .map_err(|_| denied())?
        } else if field.source_kind == "parameter_schema"
            && field.source_ref.as_deref() == Some(field.json_path.as_str())
        {
            crate::secret_values::SecretSource::ParameterSchema {
                path: field.json_path.clone(),
            }
            .provenance()
        } else {
            return Err(denied());
        };
        if provenance.origins.is_empty() {
            return Err(denied());
        }
        for origin in &provenance.origins {
            match origin {
                SecretOrigin::Local {
                    source_kind,
                    source_ref,
                } if source_kind == "parameter_schema"
                    && source_ref.as_deref() == Some(field.json_path.as_str()) =>
                {
                    // source_evidence validated this exact archived field's
                    // ciphertext, value and secret path against its contract.
                }
                SecretOrigin::Local { .. } => return Err(denied()),
                origin if super::immutable_input_origin::is_input_receipt(origin) => {}
                origin => {
                    if !ordinary.contains(origin) {
                        ordinary.push(origin.clone());
                    }
                }
            }
            if !expected.contains(origin) {
                expected.push(origin.clone());
            }
        }
        payload.merge(provenance);
    }
    if expected.as_slice() != evidence.origins() {
        return Err(denied());
    }
    let authorities = original_authorities(conn, &evidence).await?;
    let (ceiling_nodes, ceiling_bytes, ceiling_depth) = if !ordinary.is_empty() {
        if authorities.is_empty() {
            return Err(denied());
        }
        // The existing evaluator requires EVERY contributor to cover every
        // origin independently; never combine their grants into one authority.
        super::immutable_input_origin::ImmutableInputOriginRepository::require_counted(
            conn,
            &authorities,
            &ordinary,
        )
        .await?
    } else {
        (0, 0, 0)
    };
    let mut fence = context.accepted_fence().clone();
    // Execution Waits and success archives use the same retained target adapter.
    if target_kind == "execution" {
        let (original, source_fence) =
            success::execution_target(conn, target, metadata.read_owner(), source_admission)
                .await?;
        if original.release != record.authority.target_release_id
            || original.release_digest != record.authority.target_release_digest
            || original.action_ref != record.authority.binding.action_ref()
        {
            return Err(denied());
        }
        fence = fence.intersect(&source_fence);
    }
    Ok(PreparedWaitSource {
        metadata: metadata.clone(),
        evidence,
        payload,
        source_admission,
        source_execution: record.owner_id,
        release: record.authority.target_release_id,
        release_digest: record.authority.target_release_digest.clone(),
        action_ref: record.authority.binding.action_ref().to_owned(),
        ceiling_nodes,
        ceiling_bytes,
        ceiling_depth,
        transaction: sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?,
        fence,
    })
}
