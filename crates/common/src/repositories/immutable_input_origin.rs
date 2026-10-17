//! One input-purpose traversal for native renderers and worker restoration.
//! C's immutable event/sensor/log contracts are shared, never reconstructed from
//! today's secret rows. Each retained contributor must cover every origin.
use super::{
    event_origin::{EventOriginRepository, EVENT_SOURCE_CONFIG, EVENT_SOURCE_PAYLOAD},
    execution_admission::ExecutionAdmissionRepository,
    runtime_read::{
        RuntimeReadOwner, RuntimeReadRepository, RuntimeReadTarget, ENTITY_RUNTIME_READ,
    },
    runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
    FindById,
};
use crate::{
    delegation::DelegationAuthority,
    models::Id,
    rbac::Action,
    secret_provenance::{SecretOrigin, SecretProvenance},
    Error, Result,
};
use sqlx::PgConnection;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

type Node = (String, Id, String);
type Parents = Arc<BTreeMap<Node, SecretProvenance>>;

/// Direct leaf classifications and original-source descriptors from an opaque,
/// authenticated receipt. No reader grant or caller-data/serde constructor.
#[derive(Clone)]
pub struct ReceiptDisclosureLeaves {
    public: Vec<super::inherited_input::InputSite>,
    outcomes: Vec<super::inherited_input::OutcomeMetadata>,
    sources: Vec<super::inherited_input::ReceiptSourceDescriptor>,
    native: Vec<super::native_input_snapshot::NativeSnapshotMetadata>,
}

impl ReceiptDisclosureLeaves {
    pub fn has_native_sources(&self) -> bool {
        !self.native.is_empty()
    }
    pub fn from_context(context: &super::inherited_input::ReceiptContext) -> Result<Self> {
        context.ensure_fresh()?;
        Ok(Self {
            public: context.public_source_sites().to_vec(),
            outcomes: context.outcome_metadata().to_vec(),
            sources: context.source_receipts().to_vec(),
            native: context.native_source_metadata().to_vec(),
        })
    }

    /// Only direct leaves of this authenticated context may use these original
    /// sites. Never infer a descriptor from an origin ID or a current field row.
    pub fn source_receipt(
        &self,
        origin: &SecretOrigin,
    ) -> Result<&super::inherited_input::ReceiptSourceDescriptor> {
        let source = self
            .sources
            .iter()
            .find(|source| source.origin() == *origin)
            .ok_or_else(denied)?;
        source.ensure_fresh()?;
        Ok(source)
    }

    pub fn source_descriptor_bytes(&self) -> Result<usize> {
        self.sources.iter().try_fold(0usize, |bytes, source| {
            let size = serde_json::to_vec(&(
                source.retained_by(),
                source.receipt(),
                source.site(),
                source.origin(),
            ))?
            .len();
            bytes.checked_add(size).ok_or_else(denied)
        })
    }

    /// Call only AFTER current source identity and Read/Decrypt checks.
    pub fn is_public_source(&self, kind: &str, execution: Id, path: &str) -> bool {
        self.public.iter().any(|site| {
            site.kind.as_str() == kind
                && site.execution == execution
                && site.path == path
                && site.purpose == super::inherited_input::InputSitePurpose::Value
        })
    }

    /// Native status/timeout observations are not whole-result plaintext.
    pub fn is_outcome_metadata(&self, kind: &str, execution: Id, path: &str) -> bool {
        kind == "execution_result"
            && path.is_empty()
            && self
                .outcomes
                .iter()
                .any(|outcome| outcome.execution() == execution)
    }
}

/// Carries validated source deadlines through later application/commit waits.
#[derive(Default)]
pub struct ReceiptFreshness {
    contexts: Vec<super::inherited_input::ReceiptContext>,
}

impl ReceiptFreshness {
    pub fn retain(&mut self, context: super::inherited_input::ReceiptContext) -> Result<()> {
        if self.contexts.len() >= 256 {
            return Err(denied());
        }
        context.ensure_fresh()?;
        self.contexts.push(context);
        Ok(())
    }

    pub fn append(&mut self, mut other: Self) -> Result<()> {
        if self.contexts.len() + other.contexts.len() > 256 {
            return Err(denied());
        }
        self.contexts.append(&mut other.contexts);
        self.ensure_fresh()
    }

    pub fn ensure_fresh(&self) -> Result<()> {
        for context in &self.contexts {
            context.ensure_fresh()?;
        }
        Ok(())
    }
}

/// Receipt IDs are not execution IDs. Recognize the namespace even when its
/// suffix is invalid so callers cannot fall through to human/API authority.
pub fn is_input_receipt(origin: &SecretOrigin) -> bool {
    matches!(origin, SecretOrigin::Entity { entity_type, .. } if entity_type.starts_with("declared_input:"))
}

pub fn input_value_site(
    entity_type: &str,
    execution: Id,
    path: &str,
) -> Result<super::inherited_input::InputSite> {
    use super::inherited_input::{InputEntityKind, InputSite, InputSitePurpose};
    let kind = match entity_type {
        "execution_config" => InputEntityKind::ExecutionConfig,
        "execution_result" => InputEntityKind::ExecutionResult,
        "workflow_variables" => InputEntityKind::WorkflowVariables,
        _ => return Err(denied()),
    };
    Ok(InputSite {
        execution,
        kind,
        purpose: InputSitePurpose::Value,
        path: path.to_owned(),
    })
}

/// Preserve each actual delivery frame when capturing or replaying source rows.
/// Shared with the API and authenticated file-sink storage verification.
pub fn snapshot_input_rows(
    rows: &[super::execution_secret_value::SecretProvenanceRow],
) -> Result<Vec<SecretProvenance>> {
    if rows.len() > 256 {
        return Err(denied());
    }
    let mut bytes = 0usize;
    rows.iter()
        .map(|row| {
            if row.source_kind != "provenance" {
                return Err(denied());
            }
            let reference = row.source_ref.as_deref().ok_or_else(denied)?;
            bytes = bytes.checked_add(reference.len()).ok_or_else(denied)?;
            if bytes > 1024 * 1024 {
                return Err(denied());
            }
            let provenance: SecretProvenance =
                serde_json::from_str(reference).map_err(|_| denied())?;
            if provenance.origins.iter().any(is_input_receipt) {
                input_value_site(&row.entity_type, row.entity_id, &row.json_path)?;
                Ok(SecretProvenance {
                    origins: vec![SecretOrigin::Entity {
                        entity_type: row.entity_type.clone(),
                        entity_id: row.entity_id,
                        path: row.json_path.clone(),
                    }],
                    templates: vec![],
                })
            } else {
                Ok(provenance)
            }
        })
        .collect()
}

/// Native/Worker restoration consumes only the actual stored destination site.
/// Keep contexts until plaintext restoration finishes, then recheck their fences
/// in this same transaction. Remaining origins still need their ordinary checks.
pub async fn prepare_input_receipts(
    connection: &mut PgConnection,
    entity_type: &str,
    execution: Id,
    secrets: &[crate::secret_values::StoredSecretValue],
    encryption_key: &str,
) -> Result<(
    Vec<crate::secret_values::StoredSecretValue>,
    Vec<super::inherited_input::ReceiptContext>,
)> {
    use super::inherited_input::InheritedInputRepository as Inputs;
    let mut remaining = Vec::new();
    let mut contexts = Vec::new();
    if secrets.len() > 256 {
        return Err(denied());
    }
    let mut bytes = 0usize;
    for secret in secrets {
        if secret.source_kind != "provenance" {
            return Err(denied());
        }
        let reference = secret.source_ref.as_deref().ok_or_else(denied)?;
        bytes = bytes.checked_add(reference.len()).ok_or_else(denied)?;
        if bytes > 1024 * 1024 {
            return Err(denied());
        }
        let mut provenance: SecretProvenance = serde_json::from_str(reference)?;
        if provenance.origins.is_empty() || provenance.origins.len() > 256 {
            return Err(denied());
        }
        let site = input_value_site(entity_type, execution, &secret.json_path)?;
        let mut ordinary = Vec::new();
        for origin in provenance.origins {
            if !is_input_receipt(&origin) {
                ordinary.push(origin);
                continue;
            }
            if contexts.len() >= 256 {
                return Err(denied());
            }
            let receipt = Inputs::prepare_delivery(connection, &site, &origin, encryption_key)
                .await?
                .ok_or_else(denied)?;
            contexts.push(receipt);
        }
        if !ordinary.is_empty() {
            provenance.origins = ordinary;
            let mut secret = secret.clone();
            secret.source_ref = Some(serde_json::to_string(&provenance)?);
            remaining.push(secret);
        }
    }
    Ok((remaining, contexts))
}

/// After receipt AND ordinary contributor source checks, bind the actual
/// ciphertext supplied to restoration. This adds no authority by itself.
pub async fn bind_input_receipt_buffers(
    conn: &mut PgConnection,
    entity_type: &str,
    execution: Id,
    secrets: &[crate::secret_values::StoredSecretValue],
) -> Result<()> {
    if secrets.len() > 256 {
        return Err(denied());
    }
    let mut selected = Vec::new();
    let mut bytes = 0usize;
    for secret in secrets {
        let reference = secret.source_ref.as_deref().ok_or_else(denied)?;
        bytes = bytes.checked_add(reference.len()).ok_or_else(denied)?;
        if bytes > 1024 * 1024 {
            return Err(denied());
        }
        let provenance: SecretProvenance = serde_json::from_str(reference)?;
        if provenance.origins.iter().any(is_input_receipt) {
            selected.push(secret);
        }
    }
    selected.sort_unstable_by(|a, b| a.json_path.cmp(&b.json_path));
    for secret in selected {
        let stored = super::execution_secret_value::ExecutionSecretValueRepository::find_stored_at(
            conn,
            entity_type,
            execution,
            &secret.json_path,
        )
        .await?;
        if stored.as_ref() != Some(secret) {
            return Err(denied());
        }
    }
    Ok(())
}

/// Discover and authenticate receipt source fences before capture/target locks.
/// Later disclosure consumes only these prepared contexts, never another source
/// lookup behind a target. The contexts remain bound to the caller's transaction.
#[derive(Default)]
pub struct InputReceiptFrontier {
    native_waits: super::native_input_disclosure::NativeWaitSources,
    file_sinks: BTreeMap<Id, super::runtime_read::FileSinkCapture>,
    contexts: Vec<(
        super::inherited_input::InputSite,
        SecretOrigin,
        super::inherited_input::ReceiptContext,
    )>,
    sources: Vec<(
        super::inherited_input::ReceiptSourceDescriptor,
        super::inherited_input::ReceiptContext,
    )>,
}

/// Freeze only authenticated origin metadata, never a receipt's value or Key
/// bytes. Callers still authorize every source before committing the capture.
/// Observation/literal terminals below are NOT accepted local input grants:
/// replay must authenticate the retaining receipt and its exact classifications.
pub fn freeze_receipt_context(
    context: &super::inherited_input::ReceiptContext,
) -> Result<Vec<super::runtime_read::RuntimeReadParent>> {
    context.ensure_fresh()?;
    let mut parents = BTreeMap::new();
    let mut insert = |origin: SecretOrigin, provenance: SecretProvenance| -> Result<()> {
        let SecretOrigin::Entity {
            entity_type,
            entity_id,
            path,
        } = origin
        else {
            return Err(denied());
        };
        let key = (entity_type, entity_id, path);
        if parents.get(&key).is_some_and(|saved| saved != &provenance) {
            return Err(denied());
        }
        parents.insert(key, provenance);
        if parents.len() > 256 {
            return Err(denied());
        }
        Ok(())
    };
    let terminal = |source: &str| SecretProvenance {
        origins: vec![SecretOrigin::Local {
            source_kind: source.into(),
            source_ref: None,
        }],
        templates: vec![],
    };
    let provenance = if context.source_origins().is_empty() {
        terminal("receipt_literal")
    } else {
        SecretProvenance {
            origins: context.source_origins().to_vec(),
            templates: vec![],
        }
    };
    insert(context.origin(), provenance)?;
    for site in context.public_source_sites() {
        insert(
            SecretOrigin::Entity {
                entity_type: site.kind.as_str().into(),
                entity_id: site.execution,
                path: site.path.clone(),
            },
            terminal("receipt_public_source"),
        )?;
    }
    for outcome in context.outcome_metadata() {
        insert(
            SecretOrigin::Entity {
                entity_type: "execution_result".into(),
                entity_id: outcome.execution(),
                path: String::new(),
            },
            terminal("receipt_outcome_metadata"),
        )?;
    }
    let rows = parents
        .into_iter()
        .map(|((entity_type, entity_id, path), provenance)| {
            super::runtime_read::RuntimeReadParent {
                entity_type,
                entity_id,
                path,
                provenance,
            }
        })
        .collect::<Vec<_>>();
    if serde_json::to_vec(&rows)?.len() > 1024 * 1024 {
        return Err(denied());
    }
    Ok(rows)
}

impl InputReceiptFrontier {
    pub fn prepared_success_source(
        &self,
        leaves: Option<&ReceiptDisclosureLeaves>,
        ancestry: &BTreeSet<Node>,
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<super::native_input_disclosure::PreparedSuccessSource>>> {
        let direct = if let Some(leaves) = leaves {
            self.native_waits.success(&leaves.native, origin)?
        } else {
            None
        };
        let scoped = self.native_waits.success_in_scope(ancestry, origin)?;
        if direct
            .as_ref()
            .zip(scoped.as_ref())
            .is_some_and(|(a, b)| a.metadata() != b.metadata())
        {
            return Err(denied());
        }
        Ok(direct.or(scoped))
    }
    pub fn prepared_success_receipt_source(
        &self,
        leaves: &ReceiptDisclosureLeaves,
        ancestry: &BTreeSet<Node>,
        descriptor: &super::inherited_input::ReceiptSourceDescriptor,
    ) -> Result<Option<Arc<super::native_input_disclosure::PreparedSuccessSource>>> {
        self.native_waits
            .success_receipt(&leaves.native, ancestry, descriptor)
    }

    pub fn prepared_cache_source(
        &self,
        leaves: &ReceiptDisclosureLeaves,
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<super::native_input_disclosure::PreparedCacheSource>>> {
        self.native_waits.cache(&leaves.native, origin)
    }
    pub fn prepared_execution_wait(
        &self,
        leaves: &ReceiptDisclosureLeaves,
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<super::native_input_disclosure::PreparedWaitSource>>> {
        self.native_waits.execution(&leaves.native, origin)
    }
    pub fn prepared_related_wait(
        &self,
        leaves: &ReceiptDisclosureLeaves,
        origin: &SecretOrigin,
    ) -> Result<Option<Arc<super::native_input_disclosure::PreparedWaitSource>>> {
        self.native_waits.related(&leaves.native, origin)
    }
    pub fn prepared_file_sink(&self, read: Id) -> Option<&super::runtime_read::FileSinkCapture> {
        self.file_sinks.get(&read)
    }
    pub fn prepared_receipt(
        &self,
        site: &super::inherited_input::InputSite,
        origin: &SecretOrigin,
    ) -> Result<&super::inherited_input::ReceiptContext> {
        let context = self
            .contexts
            .iter()
            .find(|(saved, proof, _)| saved == site && proof == origin)
            .map(|(_, _, context)| context)
            .ok_or_else(denied)?;
        context.ensure_fresh()?;
        Ok(context)
    }
    pub async fn prepare(
        &mut self,
        conn: &mut PgConnection,
        rows: &[super::execution_secret_value::SecretProvenanceRow],
        encryption_key: Option<&str>,
    ) -> Result<()> {
        let mut pending = Vec::new();
        for row in rows {
            if row.source_kind != "provenance" {
                return Err(denied());
            }
            let provenance: SecretProvenance =
                serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?)?;
            pending.push((
                provenance,
                input_value_site(&row.entity_type, row.entity_id, &row.json_path).ok(),
                None::<Parents>,
                0usize,
                BTreeSet::<Node>::new(),
                None::<ReceiptDisclosureLeaves>,
            ));
        }
        let mut nodes = 0usize;
        let mut bytes = 0usize;
        while let Some((provenance, site, parents, depth, ancestry, leaves)) = pending.pop() {
            bytes = bytes
                .checked_add(serde_json::to_vec(&provenance)?.len())
                .ok_or_else(denied)?;
            if depth >= 32 || bytes > 1024 * 1024 {
                return Err(denied());
            }
            for origin in provenance.origins {
                nodes += 1;
                if nodes > 256 {
                    return Err(denied());
                }
                if is_input_receipt(&origin) {
                    let mut receipt_ancestry = ancestry.clone();
                    let SecretOrigin::Entity {
                        entity_type,
                        entity_id,
                        path,
                    } = &origin
                    else {
                        return Err(denied());
                    };
                    if !receipt_ancestry.insert((entity_type.clone(), *entity_id, path.clone())) {
                        return Err(denied());
                    }
                    let context = if let Some(site) = site.as_ref() {
                        let index = if let Some(index) = self
                            .contexts
                            .iter()
                            .position(|(saved, proof, _)| saved == site && proof == &origin)
                        {
                            index
                        } else {
                            if self.contexts.len() + self.sources.len() >= 256 {
                                return Err(denied());
                            }
                            let context =
                                super::inherited_input::InheritedInputRepository::receipt_context(
                                    conn,
                                    site,
                                    &origin,
                                    encryption_key.ok_or_else(denied)?,
                                )
                                .await?;
                            self.contexts.push((site.clone(), origin, context));
                            self.contexts.len() - 1
                        };
                        &self.contexts[index].2
                    } else {
                        let source = leaves
                            .as_ref()
                            .ok_or_else(denied)?
                            .source_receipt(&origin)?;
                        if let Some(success) = self.prepared_success_receipt_source(
                            leaves.as_ref().ok_or_else(denied)?,
                            &receipt_ancestry,
                            source,
                        )? {
                            receipt_ancestry.insert(success.scope_node());
                        }
                        if self.contexts.len() + self.sources.len() >= 256 {
                            return Err(denied());
                        }
                        // Revalidate the retaining graph, exact original site
                        // and ALL source fences now, before any target locks.
                        let context = super::inherited_input::InheritedInputRepository::prepare_receipt_source(
                            conn, source, encryption_key.ok_or_else(denied)?,
                        ).await?;
                        self.sources.push((source.clone(), context));
                        &self.sources.last().ok_or_else(denied)?.1
                    };
                    let underlying = SecretProvenance {
                        origins: context.source_origins().to_vec(),
                        templates: vec![],
                    };
                    if context.native_source_metadata().iter().any(|source| {
                        matches!(
                            source.source_kind(),
                            "execution_result"
                                | "inquiry_response"
                                | "queue_item_result"
                                | "cache"
                                | "ordinary_success_metadata"
                        )
                    }) {
                        let (native_nodes, native_bytes) = self
                            .native_waits
                            .prepare_context(
                                conn,
                                context,
                                encryption_key.ok_or_else(denied)?,
                                depth + 1,
                            )
                            .await?;
                        nodes = nodes.checked_add(native_nodes).ok_or_else(denied)?;
                        bytes = bytes.checked_add(native_bytes).ok_or_else(denied)?;
                        if nodes > 256 || bytes > 1024 * 1024 {
                            return Err(denied());
                        }
                    }
                    if !underlying.origins.is_empty() {
                        let classification = ReceiptDisclosureLeaves::from_context(context)?;
                        bytes = bytes
                            .checked_add(classification.source_descriptor_bytes()?)
                            .ok_or_else(denied)?;
                        if bytes > 1024 * 1024 {
                            return Err(denied());
                        }
                        pending.push((
                            underlying,
                            None,
                            parents.clone(),
                            depth + 1,
                            receipt_ancestry,
                            Some(classification),
                        ));
                    }
                    continue;
                }
                let SecretOrigin::Entity {
                    entity_type,
                    entity_id,
                    path,
                } = origin
                else {
                    continue;
                };
                let mut ancestry = ancestry.clone();
                if !ancestry.insert((entity_type.clone(), entity_id, path.clone())) {
                    return Err(denied());
                }
                let normalized_origin = SecretOrigin::Entity {
                    entity_type: entity_type.clone(),
                    entity_id,
                    path: path.clone(),
                };
                if let Some(success) =
                    self.prepared_success_source(leaves.as_ref(), &ancestry, &normalized_origin)?
                {
                    ancestry.insert(success.scope_node());
                    for row in success.rows_for(&normalized_origin)? {
                        let provenance: SecretProvenance =
                            serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?)?;
                        if provenance.origins.is_empty() {
                            continue;
                        }
                        pending.push((
                            provenance,
                            success.row_site(&row)?,
                            parents.clone(),
                            depth + 1,
                            ancestry.clone(),
                            leaves.clone(),
                        ));
                    }
                    if pending.len() + nodes > 256 {
                        return Err(denied());
                    }
                    continue;
                }
                // Classification only guides source discovery here. The API
                // evaluator still checks current source identity and rights.
                if leaves.as_ref().is_some_and(|leaves| {
                    leaves.is_public_source(&entity_type, entity_id, &path)
                        || leaves.is_outcome_metadata(&entity_type, entity_id, &path)
                }) {
                    continue;
                }
                if let Some(leaves) = &leaves {
                    let origin = SecretOrigin::Entity {
                        entity_type: entity_type.clone(),
                        entity_id,
                        path: path.clone(),
                    };
                    if self.prepared_execution_wait(leaves, &origin)?.is_some()
                        || self.prepared_related_wait(leaves, &origin)?.is_some()
                        || self.prepared_cache_source(leaves, &origin)?.is_some()
                    {
                        // A authenticated the complete archived field/contract
                        // and direct origins. They already share this queue via
                        // context.source_origins; never fetch today's result.
                        continue;
                    }
                }
                if entity_type == ENTITY_RUNTIME_READ {
                    if !path.is_empty() {
                        return Err(denied());
                    }
                    let read = RuntimeReadRepository::find(&mut *conn, entity_id).await?;
                    if !self.file_sinks.contains_key(&entity_id) {
                        if let Some(capture) =
                            RuntimeReadRepository::file_sink_capture(conn, entity_id).await?
                        {
                            bytes = bytes
                                .checked_add(capture.retained_bytes())
                                .ok_or_else(denied)?;
                            if bytes > 1024 * 1024 || self.file_sinks.len() >= 256 {
                                return Err(denied());
                            }
                            self.file_sinks.insert(entity_id, capture);
                        }
                    }
                    let frozen = Arc::new(
                        read.parents
                            .into_iter()
                            .map(|p| ((p.entity_type, p.entity_id, p.path), p.provenance))
                            .collect(),
                    );
                    if let Some(capture) = self.file_sinks.get(&entity_id) {
                        // This is source preparation only. The consumer must
                        // still match the exact admitted producer before using
                        // original input rows instead of generic read semantics.
                        for row in capture.rows() {
                            let provenance: SecretProvenance = serde_json::from_str(
                                row.source_ref.as_deref().ok_or_else(denied)?,
                            )?;
                            let site = if provenance.origins.iter().any(is_input_receipt) {
                                Some(input_value_site(
                                    &row.entity_type,
                                    row.entity_id,
                                    &row.json_path,
                                )?)
                            } else {
                                None
                            };
                            pending.push((
                                provenance,
                                site,
                                Some(Arc::clone(&frozen)),
                                depth + 1,
                                ancestry.clone(),
                                None,
                            ));
                        }
                    } else {
                        for provenance in read.provenance {
                            pending.push((
                                provenance,
                                None,
                                Some(Arc::clone(&frozen)),
                                depth + 1,
                                ancestry.clone(),
                                None,
                            ));
                        }
                    }
                } else {
                    let parent = if matches!(
                        entity_type.as_str(),
                        EVENT_SOURCE_CONFIG | EVENT_SOURCE_PAYLOAD
                    ) {
                        EventOriginRepository::evidence(&mut *conn, entity_id)
                            .await?
                            .parent(&entity_type, &path)?
                            .clone()
                    } else if let Some(parents) = &parents {
                        parents
                            .get(&(entity_type.clone(), entity_id, path.clone()))
                            .cloned()
                            .ok_or_else(denied)?
                    } else {
                        let row = super::execution_secret_value::ExecutionSecretValueRepository::find_bounded_provenance(
                            &mut *conn, &[entity_type.as_str()], &[entity_id],
                        ).await?.into_iter().find(|row| row.json_path == path).ok_or_else(denied)?;
                        if row.source_kind != "provenance" {
                            return Err(denied());
                        }
                        serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?)?
                    };
                    pending.push((
                        parent,
                        input_value_site(&entity_type, entity_id, &path).ok(),
                        parents.clone(),
                        depth + 1,
                        ancestry,
                        None,
                    ));
                }
                if pending.len() + nodes > 256 {
                    return Err(denied());
                }
            }
        }
        self.ensure_fresh_on(conn).await
    }

    pub fn source_origins(
        &self,
        site: &super::inherited_input::InputSite,
        origin: &SecretOrigin,
    ) -> Result<&[SecretOrigin]> {
        self.contexts
            .iter()
            .find(|(saved, proof, _)| saved == site && proof == origin)
            .map(|(_, _, context)| context.source_origins())
            .ok_or_else(denied)
    }

    pub fn disclosure_leaves(
        &self,
        site: &super::inherited_input::InputSite,
        origin: &SecretOrigin,
    ) -> Result<ReceiptDisclosureLeaves> {
        let context = self
            .contexts
            .iter()
            .find(|(saved, proof, _)| saved == site && proof == origin)
            .map(|(_, _, context)| context)
            .ok_or_else(denied)?;
        ReceiptDisclosureLeaves::from_context(context)
    }

    /// Pure consumption of a descriptor prepared on this frontier's original
    /// transaction. This cannot rediscover a current destination or acquire a
    /// new source lock behind the reader's targets.
    pub fn prepared_source(
        &self,
        source: &super::inherited_input::ReceiptSourceDescriptor,
    ) -> Result<&super::inherited_input::ReceiptContext> {
        source.ensure_fresh()?;
        let context = self
            .sources
            .iter()
            .find(|(saved, _)| {
                saved.retained_by() == source.retained_by()
                    && saved.receipt() == source.receipt()
                    && saved.site() == source.site()
                    && saved.origin() == source.origin()
            })
            .map(|(_, context)| context)
            .ok_or_else(denied)?;
        context.ensure_fresh()?;
        Ok(context)
    }

    pub fn ensure_fresh(&self) -> Result<()> {
        self.native_waits.ensure_fresh()?;
        for (_, _, context) in &self.contexts {
            context.ensure_fresh()?;
        }
        for (_, context) in &self.sources {
            context.ensure_fresh()?;
        }
        Ok(())
    }

    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.native_waits.ensure_held_on(conn).await?;
        for (_, _, context) in &self.contexts {
            context.ensure_fresh_on(conn).await?;
        }
        for (_, context) in &self.sources {
            context.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
}
fn denied() -> Error {
    Error::PermissionDenied(
        "Immutable input origin is not covered by every retained contributor".into(),
    )
}

pub fn is_immutable_origin(origin: &SecretOrigin) -> bool {
    is_input_receipt(origin)
        || matches!(origin, SecretOrigin::Entity { entity_type, .. } if matches!(entity_type.as_str(), ENTITY_RUNTIME_READ | EVENT_SOURCE_PAYLOAD | EVENT_SOURCE_CONFIG))
}

pub fn local_is_bound(source: &str, ancestry: &BTreeSet<Node>) -> bool {
    source == "parameter_schema"
        || source == "pack_config"
            && ancestry.iter().any(|(kind, _, _)| {
                kind == super::component_configuration::ENTITY_COMPONENT_CONFIGURATION
            })
        || source == "trigger_schema"
            && ancestry.iter().any(|(kind, _, _)| {
                matches!(
                    kind.as_str(),
                    super::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG
                        | super::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG
                        | super::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG
                        | EVENT_SOURCE_PAYLOAD
                        | EVENT_SOURCE_CONFIG
                )
            })
}

async fn target(
    conn: &mut PgConnection,
    authorities: &[DelegationAuthority],
    target: RuntimeTarget,
    actions: &[Action],
) -> Result<super::runtime_target::ResolvedRuntimeTarget> {
    let principal = authorities.first().ok_or_else(denied)?.identity_id;
    let resolved = RuntimeTargetRepository::resolve(conn, target, principal)
        .await?
        .ok_or_else(denied)?;
    if authorities.iter().any(|authority| {
        actions
            .iter()
            .any(|action| !authority.allows_input_origin_target(&resolved, *action))
    }) {
        return Err(denied());
    }
    Ok(resolved)
}

async fn execution(
    conn: &mut PgConnection,
    authorities: &[DelegationAuthority],
    id: Id,
) -> Result<()> {
    target(
        conn,
        authorities,
        RuntimeTarget::Execution(id),
        &[Action::Read, Action::Decrypt],
    )
    .await?;
    super::schema_contract::SchemaContractRepository::find(
        conn,
        super::schema_contract::BindingOwner::Execution,
        id,
    )
    .await?
    .ok_or_else(denied)?
    .validate()?;
    Ok(())
}

fn frozen_parent(
    parents: Option<&Parents>,
    kind: &str,
    id: Id,
    path: &str,
) -> Result<Option<SecretProvenance>> {
    match parents {
        Some(rows) => rows
            .get(&(kind.to_owned(), id, path.to_owned()))
            .cloned()
            .map(Some)
            .ok_or_else(denied),
        None => Ok(None),
    }
}

/// Only called for an original immutable configuration root after owner and
/// all-contributor Read/Decrypt checks. This is authorization evidence, not a
/// decryption/re-rendering path; the caller keeps the original ciphertext/key
/// checks and retained-revision lifetime. Never consult the current revision.
async fn original_configuration_parent(
    connection: &mut PgConnection,
    revision: &super::component_configuration::ConfigurationRevision,
    path: &str,
) -> Result<SecretProvenance> {
    let contract = revision.contract()?;
    let compiled = crate::value_schema::CompiledContract::new(&contract).map_err(|_| denied())?;
    if path.is_empty()
        || path.len() > 4096
        || path.split('/').count() > 33
        || !compiled
            .secret_paths(&revision.config)
            .map_err(|_| denied())?
            .iter()
            .any(|field| field == path)
        || !revision
            .config
            .pointer(path)
            .is_some_and(crate::secret_values::is_redaction_marker)
    {
        return Err(denied());
    }
    let field = super::execution_secret_value::ExecutionSecretValueRepository::find_stored_at(
        connection,
        super::component_configuration::ENTITY_COMPONENT_CONFIGURATION,
        revision.id,
        path,
    )
    .await?
    .ok_or_else(denied)?;
    if field.json_path != path
        || field.source_kind != "provenance"
        || !field.encrypted_value.is_string()
        || field.encryption_key_hash.is_none()
    {
        return Err(denied());
    }
    let reference = field.source_ref.as_deref().ok_or_else(denied)?;
    if reference.len() > 1024 * 1024 {
        return Err(denied());
    }
    let parent: SecretProvenance = serde_json::from_str(reference).map_err(|_| denied())?;
    if parent.origins.is_empty() || parent.origins.len() > 256 {
        return Err(denied());
    }
    Ok(parent)
}

pub struct ImmutableInputOriginRepository;
impl ImmutableInputOriginRepository {
    /// Contributors are accepted immutable input authorities, not API grants or
    /// current requester IAM. The caller retains their operational effect fence
    /// and this connection through the eventual restoration/publication.
    pub async fn require(
        conn: &mut PgConnection,
        authorities: &[DelegationAuthority],
        origins: &[SecretOrigin],
    ) -> Result<()> {
        Self::require_counted(conn, authorities, origins)
            .await
            .map(|_| ())
    }

    /// The same traversal, with its work returned to a retaining caller's graph
    /// budget. Native source ceilings must not reset node/byte accounting.
    pub(crate) async fn require_counted(
        conn: &mut PgConnection,
        authorities: &[DelegationAuthority],
        origins: &[SecretOrigin],
    ) -> Result<(usize, usize, usize)> {
        if origins.is_empty() {
            return Ok((0, 0, 0));
        }
        if authorities.is_empty() || origins.len() > 256 {
            return Err(denied());
        }
        let mut pending = vec![(
            SecretProvenance {
                origins: origins.to_vec(),
                templates: vec![],
            },
            None::<Parents>,
            BTreeSet::<Node>::new(),
            0usize,
            false,
        )];
        let mut nodes = 0usize;
        let mut bytes = 0usize;
        let mut sensor_frontier = super::sensor_read_origin::SensorOriginFrontier::default();
        let mut deepest = 0usize;
        while let Some((provenance, parents, ancestry, depth, schema_bound)) = pending.pop() {
            deepest = deepest.max(depth);
            bytes = bytes
                .checked_add(serde_json::to_vec(&provenance)?.len())
                .ok_or_else(denied)?;
            if depth >= 32 || bytes > 1024 * 1024 || provenance.origins.is_empty() {
                return Err(denied());
            }
            let mut known = vec![provenance.clone()];
            let mut known_bytes = serde_json::to_vec(&provenance)?.len();
            for (provenance, _, _, _, _) in &pending {
                known_bytes = known_bytes
                    .checked_add(serde_json::to_vec(provenance)?.len())
                    .ok_or_else(denied)?;
                if known_bytes > 1024 * 1024 {
                    return Err(denied());
                }
                known.push(provenance.clone());
            }
            sensor_frontier.prepare_provenance(conn, &known).await?;
            if bytes
                .checked_add(sensor_frontier.retained_bytes())
                .is_none_or(|size| size > 1024 * 1024)
            {
                return Err(denied());
            }
            for origin in provenance.origins {
                nodes += 1;
                if nodes + pending.len() > 256 {
                    return Err(denied());
                }
                match origin {
                    SecretOrigin::Key(key) => {
                        for authority in authorities {
                            for action in [Action::Read]
                                .into_iter()
                                .chain(key.encrypted.then_some(Action::Decrypt))
                            {
                                if !super::recorded_input_authority::RecordedInputAuthorityRepository::key_origin_allowed(conn,authority,&key,action).await? {
                                    return Err(denied());
                                }
                            }
                        }
                    }
                    SecretOrigin::QueueItem {
                        queue_id,
                        queue_ref,
                        pack_ref,
                        item_id,
                        ..
                    } => {
                        let item = target(
                            conn,
                            authorities,
                            RuntimeTarget::QueueItem(item_id),
                            &[Action::Read, Action::Decrypt],
                        )
                        .await?;
                        let queue = target(
                            conn,
                            authorities,
                            RuntimeTarget::Component(ComponentTarget::Queue, queue_id),
                            &[Action::Read, Action::Decrypt],
                        )
                        .await?;
                        if item.context().target_ref.as_deref() != Some(queue_ref.as_str())
                            || queue.context().target_ref.as_deref() != Some(queue_ref.as_str())
                            || queue.context().pack_ref != pack_ref
                        {
                            return Err(denied());
                        }
                    }
                    SecretOrigin::Local { source_kind, .. } => {
                        if ancestry.is_empty()
                            || !local_is_bound(&source_kind, &ancestry)
                            || source_kind == "parameter_schema" && !schema_bound
                        {
                            return Err(denied());
                        }
                    }
                    SecretOrigin::Entity {
                        entity_type,
                        entity_id,
                        path,
                    } => {
                        let mut ancestry = ancestry.clone();
                        if !ancestry.insert((entity_type.clone(), entity_id, path.clone())) {
                            return Err(denied());
                        }
                        if entity_type == ENTITY_RUNTIME_READ {
                            if !path.is_empty() {
                                return Err(denied());
                            }
                            let read = RuntimeReadRepository::find(&mut *conn, entity_id).await?;
                            let mut bound = schema_bound;
                            bytes = bytes
                                .checked_add(serde_json::to_vec(&read)?.len())
                                .ok_or_else(denied)?;
                            if bytes
                                .checked_add(sensor_frontier.retained_bytes())
                                .is_none_or(|size| size > 1024 * 1024)
                            {
                                return Err(denied());
                            }
                            match read.target {
                                RuntimeReadTarget::ExecutionMetadata { execution: id } => {
                                    target(
                                        conn,
                                        authorities,
                                        RuntimeTarget::Execution(id),
                                        &[Action::Read],
                                    )
                                    .await?;
                                }
                                RuntimeReadTarget::Execution { execution: id } => {
                                    execution(conn, authorities, id).await?;
                                    bound = true;
                                }
                                RuntimeReadTarget::LiveLog {
                                    artifact,
                                    version,
                                    execution: source,
                                    stream,
                                    owner,
                                } => {
                                    execution(conn, authorities, source).await?;
                                    bound = true;
                                    let scope = target(
                                        conn,
                                        authorities,
                                        RuntimeTarget::Artifact(artifact),
                                        &[Action::Read, Action::Decrypt],
                                    )
                                    .await?;
                                    if RuntimeReadOwner::of(&scope) != owner {
                                        return Err(denied());
                                    }
                                    RuntimeReadRepository::validate_log_target(
                                        conn, artifact, version, source, stream,
                                    )
                                    .await?;
                                }
                                RuntimeReadTarget::Artifact {
                                    artifact,
                                    owner,
                                    executions,
                                } => {
                                    nodes += executions.len();
                                    if nodes + pending.len() > 256 {
                                        return Err(denied());
                                    }
                                    for id in executions {
                                        execution(conn, authorities, id).await?;
                                        bound = true;
                                    }
                                    let scope = target(
                                        conn,
                                        authorities,
                                        RuntimeTarget::Artifact(artifact),
                                        &[Action::Read],
                                    )
                                    .await?;
                                    if RuntimeReadOwner::of(&scope) != owner {
                                        return Err(denied());
                                    }
                                }
                                RuntimeReadTarget::ArtifactVersion {
                                    artifact,
                                    version,
                                    execution: source,
                                    owner,
                                    encrypted_manifest,
                                } => {
                                    if let Some(id) = source {
                                        execution(conn, authorities, id).await?;
                                        bound = true;
                                    }
                                    let scope = target(
                                        conn,
                                        authorities,
                                        RuntimeTarget::Artifact(artifact),
                                        &[Action::Read, Action::Decrypt],
                                    )
                                    .await?;
                                    if RuntimeReadOwner::of(&scope) != owner {
                                        return Err(denied());
                                    }
                                    let row = super::artifact::ArtifactVersionRepository::find_pinned_for_share(conn,version).await?.ok_or_else(denied)?;
                                    if row.artifact != artifact || row.execution != source || matches!(row.body_state,
                                        Some(crate::models::ArtifactBodyState::Pending | crate::models::ArtifactBodyState::Deleting | crate::models::ArtifactBodyState::CleanupClaimed)) { return Err(denied()); }
                                    let actual_manifest = row
                                        .meta
                                        .as_ref()
                                        .and_then(|meta| meta.get("encrypted_log_manifest"))
                                        .and_then(serde_json::Value::as_str);
                                    let encoded = row
                                        .meta
                                        .as_ref()
                                        .and_then(|meta| meta.get("log_encoding"))
                                        .and_then(serde_json::Value::as_str)
                                        == Some(crate::runtime_read_log::ENCODING);
                                    if encrypted_manifest.as_deref() != actual_manifest
                                        || encoded && encrypted_manifest.is_none()
                                    {
                                        return Err(denied());
                                    }
                                    if encrypted_manifest.is_some() && (!encoded || !super::artifact::ArtifactVersionRepository::log_is_sealed(conn,version).await?) {
                                        return Err(denied());
                                    }
                                }
                            }
                            let frozen = Arc::new(
                                read.parents
                                    .into_iter()
                                    .map(|parent| {
                                        (
                                            (parent.entity_type, parent.entity_id, parent.path),
                                            parent.provenance,
                                        )
                                    })
                                    .collect::<BTreeMap<_, _>>(),
                            );
                            for parent in read.provenance {
                                pending.push((
                                    parent,
                                    Some(Arc::clone(&frozen)),
                                    ancestry.clone(),
                                    depth + 1,
                                    bound,
                                ));
                            }
                            if nodes + pending.len() > 256 {
                                return Err(denied());
                            }
                            continue;
                        }
                        let mut bound = schema_bound;
                        let mut original_parent = None;
                        let parent = if matches!(
                            entity_type.as_str(),
                            EVENT_SOURCE_PAYLOAD | EVENT_SOURCE_CONFIG
                        ) {
                            let evidence =
                                EventOriginRepository::evidence(&mut *conn, entity_id).await?;
                            let event = super::event::EventRepository::find_by_identity(
                                &mut *conn,
                                evidence.event,
                                evidence.created,
                            )
                            .await?
                            .ok_or_else(denied)?;
                            let scope = target(
                                conn,
                                authorities,
                                RuntimeTarget::Event {
                                    id: evidence.event,
                                    created: evidence.created,
                                },
                                &[Action::Read, Action::Decrypt],
                            )
                            .await?;
                            if !evidence.matches(&event)
                                || scope.pack_id() != evidence.managing_pack
                            {
                                return Err(denied());
                            }
                            evidence.parent(&entity_type, &path)?.clone()
                        } else {
                            match entity_type.as_str() {
                                crate::secret_values::ENTITY_EXECUTION_CONFIG
                                | crate::secret_values::ENTITY_EXECUTION_RESULT => {
                                    execution(conn, authorities, entity_id).await?;
                                    bound = true;
                                }
                                super::component_configuration::ENTITY_COMPONENT_CONFIGURATION => {
                                    let revision = super::component_configuration::ComponentConfigurationRepository::find(&mut *conn,entity_id).await?.ok_or_else(denied)?;
                                    if !super::component_configuration::ComponentConfigurationRepository::owner_survives(conn,&revision).await? { return Err(denied()); }
                                    let (kind, read) = match revision.owner_kind.as_str() {
                                        "pack" => {
                                            (RuntimeTarget::Pack(revision.owner_id), Action::Read)
                                        }
                                        "sensor" => (
                                            RuntimeTarget::Component(
                                                ComponentTarget::Sensor,
                                                revision.owner_id,
                                            ),
                                            Action::Update,
                                        ),
                                        _ => return Err(denied()),
                                    };
                                    let scope =
                                        target(conn, authorities, kind, &[read, Action::Decrypt])
                                            .await?;
                                    if scope.context().target_ref.as_deref()
                                        != Some(revision.owner_ref.as_str())
                                    {
                                        return Err(denied());
                                    }
                                    if parents.is_none() {
                                        original_parent = Some(
                                            original_configuration_parent(conn, &revision, &path)
                                                .await?,
                                        );
                                        bound = true;
                                    }
                                }
                                super::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG
                                | super::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG
                                | super::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG => {
                                    use super::sensor_schema::{
                                        ENTITY_SENSOR_GENERATION_RULE_CONFIG,
                                        ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
                                    };
                                    let (generation, rule) =
                                        if entity_type == ENTITY_SENSOR_RULE_DELIVERY_CONFIG {
                                            let (generation, id, reference, pack) = sensor_frontier
                                                .delivery(entity_id)
                                                .ok_or_else(denied)?;
                                            (generation, Some((id, reference, pack)))
                                        } else {
                                            (entity_id, None)
                                        };
                                    let binding =
                                        sensor_frontier.validated_binding(conn, generation).await?;
                                    let sensor = &binding.executable.sensor;
                                    let scope = target(
                                        conn,
                                        authorities,
                                        RuntimeTarget::Component(
                                            ComponentTarget::Sensor,
                                            sensor.id,
                                        ),
                                        &[Action::Update, Action::Decrypt],
                                    )
                                    .await?;
                                    if scope.pack_id() != sensor.pack
                                        || scope.context().target_ref.as_deref()
                                            != Some(sensor.r#ref.as_str())
                                        || scope.context().pack_ref != sensor.pack_ref
                                    {
                                        return Err(denied());
                                    }
                                    let rule =
                                        if entity_type == ENTITY_SENSOR_GENERATION_RULE_CONFIG {
                                            let id =
                                                super::sensor_read_origin::rule_id_from_path(&path)
                                                    .ok_or_else(denied)?;
                                            Some(
                                                sensor_frontier
                                                    .initial_rule(generation, id)
                                                    .ok_or_else(denied)?,
                                            )
                                        } else {
                                            rule
                                        };
                                    if let Some((id, reference, pack)) = rule {
                                        let scope = target(
                                            conn,
                                            authorities,
                                            RuntimeTarget::Component(ComponentTarget::Rule, id),
                                            &[Action::Read, Action::Decrypt],
                                        )
                                        .await?;
                                        if !sensor_frontier.rule_owner_matches(
                                            &entity_type,
                                            entity_id,
                                            id,
                                            &reference,
                                            &pack,
                                            generation,
                                            &binding,
                                            &scope,
                                        )? {
                                            return Err(denied());
                                        }
                                    }
                                }
                                crate::secret_values::ENTITY_INQUIRY_OPTIONS
                                | crate::secret_values::ENTITY_INQUIRY_RESPONSE => {
                                    let scope = target(
                                        conn,
                                        authorities,
                                        RuntimeTarget::Inquiry(entity_id),
                                        &[Action::Read, Action::Decrypt],
                                    )
                                    .await?;
                                    let inquiry = super::inquiry::InquiryRepository::find_by_id(
                                        &mut *conn, entity_id,
                                    )
                                    .await?
                                    .ok_or_else(denied)?;
                                    let schema =
                                        super::inquiry_schema::InquirySchemaRepository::find(
                                            &mut *conn, entity_id,
                                        )
                                        .await?
                                        .ok_or_else(denied)?;
                                    execution(conn, authorities, inquiry.created_by_execution)
                                        .await?;
                                    let creator =
                                        super::execution::ExecutionRepository::find_by_id(
                                            &mut *conn,
                                            inquiry.created_by_execution,
                                        )
                                        .await?
                                        .ok_or_else(denied)?;
                                    let pinned =
                                        creator.executable_snapshot.as_ref().ok_or_else(denied)?;
                                    if schema.binding.contract.source.component_ref()
                                        != creator.action_ref
                                        || pinned.executable.action.r#ref != creator.action_ref
                                        || scope.pack_id() != Some(pinned.executable.action.pack)
                                    {
                                        return Err(denied());
                                    }
                                }
                                _ => return Err(denied()),
                            }
                            match frozen_parent(parents.as_ref(), &entity_type, entity_id, &path)? {
                                Some(parent) => parent,
                                None => original_parent.ok_or_else(denied)?,
                            }
                        };
                        pending.push((parent, parents.clone(), ancestry, depth + 1, bound));
                    }
                }
            }
        }
        Ok((
            nodes,
            bytes
                .checked_add(sensor_frontier.retained_bytes())
                .ok_or_else(denied)?,
            deepest,
        ))
    }
}

impl ExecutionAdmissionRepository {
    /// Same canonical traversal as native preparation; receipt-purpose proofs
    /// remain separate and never widen contributor authority.
    pub async fn input_runtime_origin_allowed(
        conn: &mut PgConnection,
        destination: Id,
        origin: &SecretOrigin,
    ) -> Result<bool> {
        // Exact-site preparation is required for typed purpose receipts.
        if is_input_receipt(origin) {
            return Ok(false);
        }
        if !is_immutable_origin(origin) {
            return Ok(false);
        }
        let authorities = Self::input_authorities(conn, destination, None).await?;
        if authorities.is_empty() {
            return Ok(false);
        }
        let admission = Self::require(
            conn,
            super::runtime_authority::RuntimeOwner::Execution(destination),
        )
        .await?;
        let fence = super::runtime_authority::RuntimeAuthorityRepository::retained_admission_fence(
            conn,
            admission.id,
        )
        .await?;
        ImmutableInputOriginRepository::require(conn, &authorities, std::slice::from_ref(origin))
            .await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_configuration_never_repairs_a_present_frozen_parent_map() {
        let kind = super::super::component_configuration::ENTITY_COMPONENT_CONFIGURATION;
        let parent = SecretProvenance {
            origins: vec![SecretOrigin::Local {
                source_kind: "pack_config".into(),
                source_ref: Some("/token".into()),
            }],
            templates: vec![],
        };
        for key in [
            None,
            Some(("execution_config".into(), 9, "/token".into())),
            Some((kind.into(), 10, "/token".into())),
            Some((kind.into(), 9, "/other".into())),
        ] {
            let mut rows = BTreeMap::new();
            if let Some(key) = key {
                rows.insert(key, parent.clone());
            }
            assert!(matches!(
                frozen_parent(Some(&Arc::new(rows)), kind, 9, "/token"),
                Err(Error::PermissionDenied(_))
            ));
        }
        let rows = Arc::new(BTreeMap::from([(
            (kind.into(), 9, "/token".into()),
            parent.clone(),
        )]));
        assert_eq!(
            frozen_parent(Some(&rows), kind, 9, "/token").unwrap(),
            Some(parent)
        );
        assert_eq!(frozen_parent(None, kind, 9, "/token").unwrap(), None);
    }

    #[test]
    fn public_receipt_leaf_matches_only_its_exact_value_site() {
        use super::super::inherited_input::InputSitePurpose;
        // Matcher coverage only. Production classification can only be obtained
        // from A's authenticated context, never from these test-owned fields.
        let site = input_value_site("execution_config", 41, "/selector").unwrap();
        let leaves = ReceiptDisclosureLeaves {
            public: vec![site.clone()],
            outcomes: vec![],
            sources: vec![],
            native: vec![],
        };
        assert!(leaves.is_public_source("execution_config", 41, "/selector"));
        for (kind, execution, path) in [
            ("execution_result", 41, "/selector"),
            ("execution_config", 42, "/selector"),
            ("execution_config", 41, "/secret"),
            ("execution_config", 41, ""),
            (ENTITY_RUNTIME_READ, 41, "/selector"),
        ] {
            assert!(!leaves.is_public_source(kind, execution, path));
            assert!(!leaves.is_outcome_metadata(kind, execution, path));
        }
        for purpose in [InputSitePurpose::Condition, InputSitePurpose::Control] {
            let leaves = ReceiptDisclosureLeaves {
                public: vec![super::super::inherited_input::InputSite {
                    purpose,
                    ..site.clone()
                }],
                outcomes: vec![],
                sources: vec![],
                native: vec![],
            };
            assert!(!leaves.is_public_source("execution_config", 41, "/selector"));
        }
    }

    #[test]
    fn receipt_sites_keep_entity_identity_and_value_purpose() {
        use super::super::inherited_input::InputSitePurpose;
        let config = input_value_site("execution_config", 41, "/token").unwrap();
        let result = input_value_site("execution_result", 41, "/token").unwrap();
        let variables = input_value_site("workflow_variables", 41, "/token").unwrap();
        assert_ne!(config, result);
        assert_ne!(config, variables);
        assert_ne!(result, variables);
        assert_ne!(
            config,
            input_value_site("execution_config", 42, "/token").unwrap()
        );
        assert_ne!(
            config,
            input_value_site("execution_config", 41, "/other").unwrap()
        );
        assert_eq!(config.purpose, InputSitePurpose::Value);
        assert!(input_value_site("declared_input:execution_result:value", 41, "/token").is_err());
        assert!(input_value_site("event_payload", 41, "/token").is_err());
    }

    #[test]
    fn unknown_receipt_purposes_cannot_fall_through_to_ordinary_origins() {
        let origin = SecretOrigin::Entity {
            entity_type: "declared_input:execution_config:unknown".into(),
            entity_id: 9,
            path: "/token".into(),
        };
        assert!(is_input_receipt(&origin));
        assert!(is_immutable_origin(&origin));
        assert!(InputReceiptFrontier::default()
            .source_origins(
                &input_value_site("execution_config", 41, "/token").unwrap(),
                &origin,
            )
            .is_err());
    }
    #[test]
    fn bare_local_names_do_not_gain_input_purpose_from_a_generic_wrapper() {
        let mut ancestry = BTreeSet::from([(ENTITY_RUNTIME_READ.into(), 1, String::new())]);
        for name in ["pack_config", "queue_item", "trigger_schema"] {
            assert!(!local_is_bound(name, &ancestry));
        }
        ancestry.insert((
            super::super::component_configuration::ENTITY_COMPONENT_CONFIGURATION.into(),
            2,
            "/secret".into(),
        ));
        assert!(local_is_bound("pack_config", &ancestry));
        assert!(!local_is_bound("queue_item", &ancestry));
        ancestry.insert((EVENT_SOURCE_PAYLOAD.into(), 3, "/secret".into()));
        assert!(local_is_bound("trigger_schema", &ancestry));
    }

    #[test]
    fn ambiguous_event_ids_are_not_immutable_origin_evidence() {
        let origin = |entity_type: &str| SecretOrigin::Entity {
            entity_type: entity_type.into(),
            entity_id: 7,
            path: "/token".into(),
        };
        assert!(!is_immutable_origin(&origin("event_payload")));
        assert!(is_immutable_origin(&origin(EVENT_SOURCE_PAYLOAD)));
        assert!(is_immutable_origin(&origin(EVENT_SOURCE_CONFIG)));
    }
}
