//! Bounded historical-origin checks for recorded execution consumers.

use std::collections::BTreeSet;

use attune_common::{
    rbac::Action,
    repositories::{
        execution_secret_value::{ExecutionSecretValueRepository, SecretProvenanceRow},
        immutable_input_origin::ReceiptDisclosureLeaves,
        runtime_authority::ValidatedRuntimeAuthority,
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
        FindById,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{ENTITY_EVENT_PAYLOAD, ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT},
};
use sqlx::PgConnection;

use crate::middleware::{ApiError, ApiResult};

const MAX_ORIGIN_NODES: usize = 256;
const MAX_ORIGIN_DEPTH: usize = 32;
const MAX_ORIGIN_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(crate) struct OriginBudget {
    nodes: usize,
    bytes: usize,
    parents: std::collections::BTreeMap<(String, i64, String), SecretProvenance>,
    sensor_frontier: attune_common::repositories::sensor_read_origin::SensorOriginFrontier,
    encryption_key: Option<String>,
    receipt_frontier: attune_common::repositories::immutable_input_origin::InputReceiptFrontier,
}

impl OriginBudget {
    pub(crate) fn ensure_receipts_fresh(&self) -> ApiResult<()> {
        self.receipt_frontier.ensure_fresh()?;
        Ok(())
    }
    pub(crate) async fn ensure_receipts_fresh_on(&self, conn: &mut PgConnection) -> ApiResult<()> {
        self.receipt_frontier.ensure_fresh_on(conn).await?;
        Ok(())
    }
    pub(crate) async fn prepare_receipts(
        &mut self,
        conn: &mut PgConnection,
        rows: &[SecretProvenanceRow],
    ) -> ApiResult<()> {
        self.receipt_frontier
            .prepare(conn, rows, self.encryption_key.as_deref())
            .await
            .map_err(|error| tag_error(OriginDenialStage::PrepareSources, error.into()))?;
        Ok(())
    }
    pub(crate) fn with_encryption_key(key: Option<&str>) -> Self {
        Self {
            encryption_key: key.map(str::to_owned),
            ..Self::default()
        }
    }
    pub(crate) fn retained_parents(
        &self,
    ) -> Vec<attune_common::repositories::runtime_read::RuntimeReadParent> {
        self.parents
            .iter()
            .map(|((entity_type, entity_id, path), provenance)| {
                attune_common::repositories::runtime_read::RuntimeReadParent {
                    entity_type: entity_type.clone(),
                    entity_id: *entity_id,
                    path: path.clone(),
                    provenance: provenance.clone(),
                }
            })
            .collect()
    }
}

fn denied() -> ApiError {
    ApiError::Forbidden("Recorded secret-origin disclosure is unavailable".into())
}

/// Static internal diagnostics only. Never attach origins, paths, IDs, values,
/// error bodies or provenance to these events or to the public response.
#[derive(Clone, Copy, Debug)]
enum OriginDenialStage {
    PrepareSources,
    CaptureRows,
    TargetUnavailable,
    TargetPermission,
    ReceiptContext,
    ReceiptDescriptor,
    ReceiptParentConflict,
    LocalOrigin,
    ReadExecutionTarget,
    SourceExecutionTarget,
    NativeWaitReader,
    NativeSuccessReader,
    NativeWaitCompletionReader,
    NativeCacheReader,
    ReadArtifactTarget,
    ReadVersionUnavailable,
    ReadVersionPending,
    ReadOwnerMismatch,
    FrozenParentMissing,
    FrozenParentConflict,
    UnknownEntity,
    Bounds,
}

fn tag_error(stage: OriginDenialStage, error: ApiError) -> ApiError {
    tracing::warn!(origin_denial_stage = ?stage, "Recorded origin authorization denied");
    error
}

fn denied_at(stage: OriginDenialStage) -> ApiError {
    tag_error(stage, denied())
}

/// A generic read never upgrades legacy local display names into source proof.
/// These ancestor entries exist only after the corresponding typed source gate.
pub(crate) fn local_origin_allowed(
    source_kind: &str,
    ancestry: &BTreeSet<(String, i64, String)>,
) -> bool {
    source_kind == "parameter_schema"
        || source_kind == "pack_config" && ancestry.iter().any(|(kind, _, _)| kind == attune_common::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION)
        || source_kind == "trigger_schema" && ancestry.iter().any(|(kind, _, _)| matches!(kind.as_str(),
            attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG
            | attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG
            | attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG
            | attune_common::repositories::event_origin::EVENT_SOURCE_PAYLOAD
            | attune_common::repositories::event_origin::EVENT_SOURCE_CONFIG))
}

pub(crate) async fn execution_origins(
    conn: &mut PgConnection,
    execution: i64,
) -> ApiResult<Vec<SecretProvenanceRow>> {
    let mut rows = ExecutionSecretValueRepository::find_bounded_provenance(
        &mut *conn,
        &[
            ENTITY_EXECUTION_CONFIG,
            ENTITY_EXECUTION_RESULT,
            "workflow_variables",
        ],
        &[execution],
    )
    .await?;
    for (_, origins) in attune_common::repositories::execution_key_read::ExecutionKeyReadRepository::origins_for_executions(&mut *conn, &[execution]).await? {
        for (index, origin) in origins.into_iter().enumerate() {
            rows.push(SecretProvenanceRow {
                entity_type: "execution_runtime_key_read".into(), entity_id: execution,
                json_path: format!("/{index}"), source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&SecretProvenance::key(origin)).map_err(|_| denied())?),
            });
        }
    }
    if rows.len() > MAX_ORIGIN_NODES {
        return Err(denied());
    }
    for origin in
        attune_common::repositories::runtime_read::RuntimeReadRepository::references_for_executions(
            &mut *conn,
            &[execution],
        )
        .await?
        .remove(&execution)
        .unwrap_or_default()
    {
        rows.push(SecretProvenanceRow {
            entity_type: attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ.into(),
            entity_id: execution,
            json_path: String::new(),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(&SecretProvenance {
                    origins: vec![origin],
                    templates: vec![],
                })
                .map_err(|_| denied())?,
            ),
        });
    }
    if rows.len() > MAX_ORIGIN_NODES {
        return Err(denied());
    }
    Ok(rows)
}

pub(crate) fn snapshot(rows: &[SecretProvenanceRow]) -> ApiResult<Vec<SecretProvenance>> {
    attune_common::repositories::immutable_input_origin::snapshot_input_rows(rows)
        .map_err(|_| denied_at(OriginDenialStage::CaptureRows))
}

pub(crate) async fn require_target(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    target: RuntimeTarget,
    actions: &[Action],
) -> ApiResult<attune_common::repositories::runtime_target::ResolvedRuntimeTarget> {
    let resolved = RuntimeTargetRepository::resolve(
        conn,
        target,
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| denied_at(OriginDenialStage::TargetUnavailable))?;
    if actions
        .iter()
        .any(|action| !authority.allows_target(&resolved, *action))
    {
        return Err(denied_at(OriginDenialStage::TargetPermission));
    }
    authority.ensure_fresh_on(conn).await?;
    Ok(resolved)
}

/// Paths are followed under one credential fence. Nothing in this traversal
/// consults current requester/issuer IAM or infers pack ownership from a ref.
pub(crate) async fn authorize_origins(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    rows: &[SecretProvenanceRow],
) -> ApiResult<()> {
    authorize_origins_bounded(conn, authority, rows, &mut OriginBudget::default()).await
}

pub(crate) async fn authorize_origins_with_key(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    rows: &[SecretProvenanceRow],
    key: Option<&str>,
) -> ApiResult<OriginBudget> {
    let mut budget = OriginBudget::with_encryption_key(key);
    budget.prepare_receipts(conn, rows).await?;
    attune_common::repositories::runtime_read::RuntimeReadRepository::lock_open(
        conn,
        authority.credential_binding().admission_id,
    )
    .await?;
    authorize_origins_bounded(conn, authority, rows, &mut budget).await?;
    Ok(budget)
}

pub(crate) async fn authorize_origins_bounded(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    rows: &[SecretProvenanceRow],
    budget: &mut OriginBudget,
) -> ApiResult<()> {
    if rows.len() > MAX_ORIGIN_NODES {
        return Err(denied());
    }
    let mut pending = Vec::new();
    let mut receipt_parents = std::collections::BTreeMap::new();
    let mut success_archives = BTreeSet::new();
    let mut wait_completion_archives = BTreeSet::new();
    for row in rows {
        let provenance: SecretProvenance =
            serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?)
                .map_err(|_| denied())?;
        let site = if provenance
            .origins
            .iter()
            .any(attune_common::repositories::immutable_input_origin::is_input_receipt)
        {
            let site = attune_common::repositories::immutable_input_origin::input_value_site(
                &row.entity_type,
                row.entity_id,
                &row.json_path,
            )
            .map_err(|_| denied())?;
            let key = (
                row.entity_type.clone(),
                row.entity_id,
                row.json_path.clone(),
            );
            if budget
                .parents
                .get(&key)
                .is_some_and(|saved| saved != &provenance)
            {
                return Err(denied());
            }
            budget.parents.insert(key, provenance);
            Some(site)
        } else {
            None
        };
        pending.push((
            row.source_kind.clone(),
            row.source_ref.clone(),
            0usize,
            BTreeSet::<(String, i64, String)>::new(),
            None::<std::collections::BTreeMap<(String, i64, String), SecretProvenance>>,
            site,
            None::<ReceiptDisclosureLeaves>,
        ));
    }
    let OriginBudget {
        nodes,
        bytes,
        parents: captured_parents,
        sensor_frontier,
        receipt_frontier,
        ..
    } = budget;
    while let Some((kind, reference, depth, ancestry, retained, site, leaves)) = pending.pop() {
        *nodes += 1;
        if *nodes > MAX_ORIGIN_NODES || depth >= MAX_ORIGIN_DEPTH || kind != "provenance" {
            return Err(denied_at(OriginDenialStage::Bounds));
        }
        let reference = reference.ok_or_else(denied)?;
        *bytes = bytes.checked_add(reference.len()).ok_or_else(denied)?;
        if *bytes > MAX_ORIGIN_BYTES {
            return Err(denied());
        }
        let provenance: SecretProvenance =
            serde_json::from_str(&reference).map_err(|_| denied())?;
        if provenance.origins.is_empty() || provenance.origins.len() + *nodes > MAX_ORIGIN_NODES {
            return Err(denied());
        }
        let mut known = vec![provenance.clone()];
        let mut known_bytes = reference.len();
        for (kind, reference, _, _, _, _, _) in &pending {
            if kind == "provenance" {
                let reference = reference.as_deref().ok_or_else(denied)?;
                known_bytes = known_bytes
                    .checked_add(reference.len())
                    .ok_or_else(denied)?;
                if known_bytes > MAX_ORIGIN_BYTES {
                    return Err(denied());
                }
                known.push(serde_json::from_str(reference).map_err(|_| denied())?);
            }
        }
        sensor_frontier.prepare_provenance(conn, &known).await?;
        if bytes
            .checked_add(sensor_frontier.retained_bytes())
            .is_none_or(|bytes| bytes > MAX_ORIGIN_BYTES)
        {
            return Err(denied());
        }
        for origin in provenance.origins {
            *nodes += 1;
            if attune_common::repositories::immutable_input_origin::is_input_receipt(&origin) {
                let context = if let Some(site) = site.as_ref() {
                    receipt_frontier
                        .prepared_receipt(site, &origin)
                        .map_err(|error| {
                            tag_error(OriginDenialStage::ReceiptContext, error.into())
                        })?
                } else {
                    let source = leaves
                        .as_ref()
                        .ok_or_else(|| denied_at(OriginDenialStage::ReceiptDescriptor))?
                        .source_receipt(&origin)
                        .map_err(|error| {
                            tag_error(OriginDenialStage::ReceiptDescriptor, error.into())
                        })?;
                    receipt_frontier.prepared_source(source).map_err(|error| {
                        tag_error(OriginDenialStage::ReceiptContext, error.into())
                    })?
                };
                for parent in
                    attune_common::repositories::immutable_input_origin::freeze_receipt_context(
                        context,
                    )?
                {
                    let key = (
                        parent.entity_type.clone(),
                        parent.entity_id,
                        parent.path.clone(),
                    );
                    if let Some(saved) = receipt_parents.get(&key) {
                        if saved != &parent.provenance {
                            return Err(denied_at(OriginDenialStage::ReceiptParentConflict));
                        }
                    } else {
                        *bytes = bytes
                            .checked_add(
                                serde_json::to_vec(&(&key, &parent.provenance))
                                    .map_err(|_| denied())?
                                    .len(),
                            )
                            .ok_or_else(denied)?;
                        *nodes += 1;
                        if *bytes > MAX_ORIGIN_BYTES || *nodes > MAX_ORIGIN_NODES {
                            return Err(denied());
                        }
                        receipt_parents.insert(key, parent.provenance);
                    }
                }
                let mut source_ancestry = ancestry.clone();
                let (origins, classification) = if let Some(site) = site.as_ref() {
                    (
                        receipt_frontier.source_origins(site, &origin)?.to_vec(),
                        receipt_frontier.disclosure_leaves(site, &origin)?,
                    )
                } else {
                    let source = leaves
                        .as_ref()
                        .ok_or_else(denied)?
                        .source_receipt(&origin)?;
                    let context = receipt_frontier.prepared_source(source)?;
                    let origins = context.source_origins().to_vec();
                    let classification = ReceiptDisclosureLeaves::from_context(context)?;
                    // Source purpose authority is not a reader grant. Check
                    // the authenticated ORIGINAL execution, even if this leaf's
                    // closure contains only Keys and no execution wrapper.
                    let execution = source.site().execution;
                    if let Some(wait_completion) = receipt_frontier
                        .prepared_wait_completion_receipt_source(
                            leaves.as_ref().ok_or_else(denied)?,
                            &source_ancestry,
                            source,
                        )?
                    {
                        authorize_wait_completion_source(
                            conn,
                            authority,
                            &wait_completion,
                            retained.as_ref(),
                            captured_parents,
                            &mut wait_completion_archives,
                            nodes,
                            bytes,
                        )
                        .await?;
                        source_ancestry.insert(wait_completion.scope_node());
                    } else if let Some(success) = receipt_frontier.prepared_success_receipt_source(
                        leaves.as_ref().ok_or_else(denied)?,
                        &source_ancestry,
                        source,
                    )? {
                        authorize_success_source(
                            conn,
                            authority,
                            &success,
                            retained.as_ref(),
                            captured_parents,
                            &mut success_archives,
                            nodes,
                            bytes,
                        )
                        .await?;
                        source_ancestry.insert(success.scope_node());
                    } else {
                        require_target(
                            conn,
                            authority,
                            RuntimeTarget::Execution(execution),
                            &[Action::Read, Action::Decrypt],
                        )
                        .await?;
                        if attune_common::repositories::schema_contract::SchemaContractRepository::find(
                        &mut *conn,
                        attune_common::repositories::schema_contract::BindingOwner::Execution,
                        execution,
                    )
                    .await?
                    .is_none()
                    {
                        return Err(denied());
                    }
                    }
                    (origins, classification)
                };
                *bytes = bytes
                    .checked_add(classification.source_descriptor_bytes()?)
                    .ok_or_else(denied)?;
                if *bytes > MAX_ORIGIN_BYTES {
                    return Err(denied());
                }
                let provenance = SecretProvenance {
                    origins,
                    templates: vec![],
                };
                // Receipt validation is purpose authority only. Every original
                // source still passes this reader's current disclosure policy.
                if !provenance.origins.is_empty() {
                    pending.push((
                        "provenance".into(),
                        Some(serde_json::to_string(&provenance).map_err(|_| denied())?),
                        depth + 1,
                        source_ancestry,
                        retained.clone(),
                        None,
                        Some(classification),
                    ));
                }
                continue;
            }
            match origin {
                SecretOrigin::Key(origin) => {
                    let actions = if origin.encrypted {
                        &[Action::Read, Action::Decrypt][..]
                    } else {
                        &[Action::Read][..]
                    };
                    let mut ownership =
                        attune_common::repositories::iam_ownership::VerifiedKeyOrigins::default();
                    let target = ownership
                        .retain_recorded(
                            conn,
                            authority.credential_binding().sponsor_identity_id,
                            &origin,
                        )
                        .await?
                        .ok_or_else(denied)?;
                    if actions
                        .iter()
                        .any(|action| !authority.allows_target(&target, *action))
                    {
                        return Err(denied());
                    }
                }
                SecretOrigin::QueueItem {
                    queue_id,
                    queue_ref,
                    pack_ref,
                    ..
                } => {
                    let target = require_target(
                        conn,
                        authority,
                        RuntimeTarget::Component(ComponentTarget::Queue, queue_id),
                        &[Action::Read, Action::Decrypt],
                    )
                    .await?;
                    if target.context().target_ref.as_deref() != Some(queue_ref.as_str())
                        || target.context().pack_ref != pack_ref
                    {
                        return Err(denied());
                    }
                }
                SecretOrigin::Local { source_kind, .. } => {
                    if !local_origin_allowed(&source_kind, &ancestry) {
                        return Err(denied_at(OriginDenialStage::LocalOrigin));
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
                    let normalized_origin = SecretOrigin::Entity {
                        entity_type: entity_type.clone(),
                        entity_id,
                        path: path.clone(),
                    };
                    if let Some(wait_completion) = receipt_frontier
                        .prepared_wait_completion_source(
                            leaves.as_ref(),
                            &ancestry,
                            &normalized_origin,
                        )?
                    {
                        authorize_wait_completion_source(
                            conn,
                            authority,
                            &wait_completion,
                            retained.as_ref(),
                            captured_parents,
                            &mut wait_completion_archives,
                            nodes,
                            bytes,
                        )
                        .await?;
                        ancestry.insert(wait_completion.scope_node());
                        for row in wait_completion.rows_for(&normalized_origin)? {
                            let provenance: SecretProvenance =
                                serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?)
                                    .map_err(|_| denied())?;
                            if provenance.origins.is_empty() {
                                continue;
                            }
                            let site = wait_completion.row_site(&row)?;
                            pending.push((
                                row.source_kind,
                                row.source_ref,
                                depth + 1,
                                ancestry.clone(),
                                retained.clone(),
                                site,
                                leaves.clone(),
                            ));
                        }
                        if pending.len() + *nodes > MAX_ORIGIN_NODES {
                            return Err(denied_at(OriginDenialStage::Bounds));
                        }
                        continue;
                    }
                    if let Some(success) = receipt_frontier.prepared_success_source(
                        leaves.as_ref(),
                        &ancestry,
                        &normalized_origin,
                    )? {
                        authorize_success_source(
                            conn,
                            authority,
                            &success,
                            retained.as_ref(),
                            captured_parents,
                            &mut success_archives,
                            nodes,
                            bytes,
                        )
                        .await?;
                        ancestry.insert(success.scope_node());
                        for row in success.rows_for(&normalized_origin)? {
                            let provenance: SecretProvenance =
                                serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?)
                                    .map_err(|_| denied())?;
                            if provenance.origins.is_empty() {
                                continue;
                            }
                            let site = success.row_site(&row)?;
                            pending.push((
                                row.source_kind,
                                row.source_ref,
                                depth + 1,
                                ancestry.clone(),
                                retained.clone(),
                                site,
                                leaves.clone(),
                            ));
                        }
                        if pending.len() + *nodes > MAX_ORIGIN_NODES {
                            return Err(denied_at(OriginDenialStage::Bounds));
                        }
                        continue;
                    }
                    if let Some(leaves) = &leaves {
                        let origin = SecretOrigin::Entity {
                            entity_type: entity_type.clone(),
                            entity_id,
                            path: path.clone(),
                        };
                        if let Some(source) =
                            receipt_frontier.prepared_cache_source(leaves, &origin)?
                        {
                            *nodes = nodes
                                .checked_add(1)
                                .ok_or_else(|| denied_at(OriginDenialStage::Bounds))?;
                            *bytes = bytes
                                .checked_add(source.retained_bytes())
                                .ok_or_else(|| denied_at(OriginDenialStage::Bounds))?;
                            if *nodes > MAX_ORIGIN_NODES || *bytes > MAX_ORIGIN_BYTES {
                                return Err(denied_at(OriginDenialStage::Bounds));
                            }
                            let target = RuntimeTargetRepository::resolve_native_cache_source(
                                conn,
                                &source,
                                authority.credential_binding().sponsor_identity_id,
                            )
                            .await?;
                            if [Action::Read, Action::Decrypt]
                                .into_iter()
                                .any(|action| !authority.allows_target(&target, action))
                            {
                                return Err(denied_at(OriginDenialStage::NativeCacheReader));
                            }
                            let parent = source.frozen_parent();
                            let key = (parent.entity_type, parent.entity_id, parent.path);
                            if let Some(retained) = &retained {
                                if retained.get(&key) != Some(&parent.provenance) {
                                    return Err(denied_at(OriginDenialStage::FrozenParentConflict));
                                }
                            } else if let Some(saved) = captured_parents.get(&key) {
                                if saved != &parent.provenance {
                                    return Err(denied_at(OriginDenialStage::FrozenParentConflict));
                                }
                            } else {
                                if captured_parents.len() >= MAX_ORIGIN_NODES {
                                    return Err(denied_at(OriginDenialStage::Bounds));
                                }
                                captured_parents.insert(key, parent.provenance);
                            }
                            source.ensure_fresh()?;
                            continue;
                        }
                        if let Some(source) = receipt_frontier
                            .prepared_execution_wait(leaves, &origin)?
                            .or(receipt_frontier.prepared_related_wait(leaves, &origin)?)
                        {
                            *nodes = nodes
                                .checked_add(source.retained_nodes())
                                .ok_or_else(|| denied_at(OriginDenialStage::Bounds))?;
                            *bytes = bytes
                                .checked_add(source.retained_bytes())
                                .ok_or_else(|| denied_at(OriginDenialStage::Bounds))?;
                            if *nodes > MAX_ORIGIN_NODES || *bytes > MAX_ORIGIN_BYTES {
                                return Err(denied_at(OriginDenialStage::Bounds));
                            }
                            let principal = authority.credential_binding().sponsor_identity_id;
                            let target = if source.metadata().source_kind() == "execution_result" {
                                RuntimeTargetRepository::resolve_execution_wait_source(
                                    conn, &source, principal,
                                )
                                .await?
                            } else {
                                RuntimeTargetRepository::resolve_related_wait_source(
                                    conn, &source, principal,
                                )
                                .await?
                            };
                            if [Action::Read, Action::Decrypt]
                                .into_iter()
                                .any(|action| !authority.allows_target(&target, action))
                            {
                                return Err(denied_at(OriginDenialStage::NativeWaitReader));
                            }
                            let parent = source.frozen_parent();
                            let key = (parent.entity_type, parent.entity_id, parent.path);
                            if let Some(retained) = &retained {
                                if retained.get(&key) != Some(&parent.provenance) {
                                    return Err(denied_at(OriginDenialStage::FrozenParentConflict));
                                }
                            } else if let Some(saved) = captured_parents.get(&key) {
                                if saved != &parent.provenance {
                                    return Err(denied_at(OriginDenialStage::FrozenParentConflict));
                                }
                            } else {
                                if captured_parents.len() >= MAX_ORIGIN_NODES {
                                    return Err(denied_at(OriginDenialStage::Bounds));
                                }
                                captured_parents.insert(key, parent.provenance);
                            }
                            // A placed all exact archived field origins on this
                            // same queue. Their current reader checks continue;
                            // no current result/schema lookup or plaintext rebase.
                            continue;
                        }
                    }
                    let mut immutable_parent = None;
                    match entity_type.as_str() {
                        attune_common::repositories::event_origin::EVENT_SOURCE_PAYLOAD
                        | attune_common::repositories::event_origin::EVENT_SOURCE_CONFIG => {
                            use attune_common::repositories::{event_origin::EventOriginRepository, event::EventRepository};
                            let source = EventOriginRepository::evidence(&mut *conn, entity_id).await?;
                            let event = EventRepository::find_by_identity(&mut *conn, source.event, source.created).await?.ok_or_else(denied)?;
                            if !source.matches(&event) { return Err(denied()); }
                            let target = require_target(conn, authority, RuntimeTarget::Event { id: source.event, created: source.created }, &[Action::Read, Action::Decrypt]).await?;
                            if target.pack_id() != source.managing_pack { return Err(denied()); }
                            immutable_parent = Some(source.parent(&entity_type, &path)?.clone());
                        }
                        attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ => {
                            use attune_common::repositories::{runtime_read::{RuntimeReadRepository, RuntimeReadTarget}, artifact::ArtifactVersionRepository};
                            if !path.is_empty() { return Err(denied()); }
                            let read = RuntimeReadRepository::find(&mut *conn, entity_id).await?;
                            if let Some(capture) = receipt_frontier.prepared_file_sink(entity_id)
                                .filter(|capture| capture.is_for(&authority.credential_binding()))
                            {
                                // This registry node is this producer's OUTPUT
                                // capture, not an SDK read of the pending sink.
                                // Replay the original immutable source rows and
                                // their exact delivery sites. F still authorizes
                                // the mutation and exact version independently.
                                let frozen = read.parents.into_iter().map(|parent| ((parent.entity_type, parent.entity_id, parent.path), parent.provenance)).collect::<std::collections::BTreeMap<_, _>>();
                                for row in capture.rows() {
                                    let provenance: SecretProvenance = serde_json::from_str(row.source_ref.as_deref().ok_or_else(|| denied_at(OriginDenialStage::CaptureRows))?)
                                        .map_err(|_| denied_at(OriginDenialStage::CaptureRows))?;
                                    let site = if provenance.origins.iter().any(attune_common::repositories::immutable_input_origin::is_input_receipt) {
                                        Some(attune_common::repositories::immutable_input_origin::input_value_site(&row.entity_type, row.entity_id, &row.json_path)
                                            .map_err(|_| denied_at(OriginDenialStage::CaptureRows))?)
                                    } else { None };
                                    pending.push((row.source_kind.clone(), row.source_ref.clone(), depth + 1, ancestry.clone(), Some(frozen.clone()), site, None));
                                }
                                if pending.len() + *nodes > MAX_ORIGIN_NODES { return Err(denied_at(OriginDenialStage::Bounds)); }
                                continue;
                            }
                            match read.target {
                                RuntimeReadTarget::ExecutionMetadata { execution } => {
                                    require_target(conn, authority, RuntimeTarget::Execution(execution), &[Action::Read]).await?;
                                }
                                RuntimeReadTarget::LiveLog { artifact, version, execution, stream, owner } => {
                                    require_target(conn, authority, RuntimeTarget::Execution(execution), &[Action::Read, Action::Decrypt]).await.map_err(|error| tag_error(OriginDenialStage::ReadExecutionTarget, error))?;
                                    let target = require_target(conn, authority, RuntimeTarget::Artifact(artifact), &[Action::Read, Action::Decrypt]).await?;
                                    if attune_common::repositories::runtime_read::RuntimeReadOwner::of(&target) != owner { return Err(denied()); }
                                    RuntimeReadRepository::validate_log_target(conn, artifact, version, execution, stream).await?;
                                }
                                RuntimeReadTarget::Execution { execution } => {
                                    require_target(conn, authority, RuntimeTarget::Execution(execution), &[Action::Read, Action::Decrypt]).await.map_err(|error| tag_error(OriginDenialStage::ReadExecutionTarget, error))?;
                                    if attune_common::repositories::schema_contract::SchemaContractRepository::find(&mut *conn,
                                        attune_common::repositories::schema_contract::BindingOwner::Execution, execution).await?.is_none() { return Err(denied()); }
                                }
                                RuntimeReadTarget::Artifact { artifact, owner, executions } => {
                                    *nodes += executions.len();
                                    if *nodes > MAX_ORIGIN_NODES { return Err(denied()); }
                                    for execution in executions {
                                        require_target(conn, authority, RuntimeTarget::Execution(execution), &[Action::Read, Action::Decrypt]).await.map_err(|error| tag_error(OriginDenialStage::ReadExecutionTarget, error))?;
                                    }
                                    let target = require_target(conn, authority, RuntimeTarget::Artifact(artifact), &[Action::Read]).await?;
                                    if attune_common::repositories::runtime_read::RuntimeReadOwner::of(&target) != owner { return Err(denied()); }
                                }
                                RuntimeReadTarget::ArtifactVersion { artifact, version, execution, owner, encrypted_manifest } => {
                                    if let Some(execution) = execution {
                                        require_target(conn, authority, RuntimeTarget::Execution(execution), &[Action::Read, Action::Decrypt]).await.map_err(|error| tag_error(OriginDenialStage::ReadExecutionTarget, error))?;
                                    }
                                    let target = require_target(conn, authority, RuntimeTarget::Artifact(artifact), &[Action::Read, Action::Decrypt]).await.map_err(|error| tag_error(OriginDenialStage::ReadArtifactTarget, error))?;
                                    if attune_common::repositories::runtime_read::RuntimeReadOwner::of(&target) != owner { return Err(denied_at(OriginDenialStage::ReadOwnerMismatch)); }
                                    let row = ArtifactVersionRepository::find_pinned_for_share(conn, version).await?.ok_or_else(|| denied_at(OriginDenialStage::ReadVersionUnavailable))?;
                                    if row.artifact != artifact || row.execution != execution { return Err(denied()); }
                                    if let Some(manifest) = encrypted_manifest {
                                        if row.meta.as_ref().and_then(|meta| meta.get("encrypted_log_manifest")).and_then(serde_json::Value::as_str) != Some(manifest.as_str())
                                            || row.meta.as_ref().and_then(|meta| meta.get("log_encoding")).and_then(serde_json::Value::as_str) != Some(attune_common::runtime_read_log::ENCODING)
                                            || !ArtifactVersionRepository::log_is_sealed(conn, version).await? { return Err(denied()); }
                                    }
                                    match row.body_state {
                                        Some(attune_common::models::ArtifactBodyState::Pending) => return Err(denied_at(OriginDenialStage::ReadVersionPending)),
                                        Some(attune_common::models::ArtifactBodyState::Deleting | attune_common::models::ArtifactBodyState::CleanupClaimed) => return Err(denied_at(OriginDenialStage::ReadVersionUnavailable)),
                                        _ => {}
                                    }
                                }
                            }
                            let frozen = read.parents.into_iter().map(|parent| ((parent.entity_type, parent.entity_id, parent.path), parent.provenance)).collect::<std::collections::BTreeMap<_, _>>();
                            for provenance in read.provenance {
                                pending.push(("provenance".into(), Some(serde_json::to_string(&provenance).map_err(|_| denied())?), depth + 1, ancestry.clone(), Some(frozen.clone()), None, None));
                            }
                            if pending.len() + *nodes > MAX_ORIGIN_NODES { return Err(denied()); }
                            continue;
                        }
                        ENTITY_EXECUTION_CONFIG | ENTITY_EXECUTION_RESULT | "workflow_variables" => {
                            require_target(conn, authority, RuntimeTarget::Execution(entity_id), &[Action::Read, Action::Decrypt]).await.map_err(|error| tag_error(OriginDenialStage::SourceExecutionTarget, error))?;
                            if attune_common::repositories::schema_contract::SchemaContractRepository::find(
                                &mut *conn, attune_common::repositories::schema_contract::BindingOwner::Execution, entity_id,
                            ).await?.is_none() { return Err(denied()); }
                        }
                        ENTITY_EVENT_PAYLOAD => {
                            // Legacy Entity origins retain only event.id. It is
                            // not globally unique across partitions, and a later
                            // lookup cannot prove the captured created identity.
                            // Keep this precise origin closed until producers
                            // retain (id,created) in the immutable source record.
                            return Err(denied());
                        }
                        attune_common::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION => {
                            let revisions = attune_common::repositories::component_configuration::ComponentConfigurationRepository::find_disclosure_rows(&mut *conn, &[entity_id]).await?;
                            let revision = revisions.first().ok_or_else(denied)?;
                            if !revision.owner_survives { return Err(denied()); }
                            let (target, read) = if revision.owner_kind == "pack" {
                                (RuntimeTarget::Pack(revision.owner_id), Action::Read)
                            } else if revision.owner_kind == "sensor" {
                                (RuntimeTarget::Component(ComponentTarget::Sensor, revision.owner_id), Action::Update)
                            } else { return Err(denied()); };
                            let target = require_target(conn, authority, target, &[read, Action::Decrypt]).await?;
                            if target.context().target_ref.as_deref() != Some(revision.owner_ref.as_str()) { return Err(denied()); }
                        }
                        attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG
                        | attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG
                        | attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG => {
                            use attune_common::repositories::sensor_schema::{ENTITY_SENSOR_RULE_DELIVERY_CONFIG, ENTITY_SENSOR_GENERATION_RULE_CONFIG};
                            let (generation, rule) = if entity_type == ENTITY_SENSOR_RULE_DELIVERY_CONFIG {
                                let (generation, id, reference, pack) = sensor_frontier.delivery(entity_id).ok_or_else(denied)?;
                                (generation, Some((id, reference, pack)))
                            } else { (entity_id, None) };
                            let binding = sensor_frontier.validated_binding(conn, generation).await?;
                            let sensor = &binding.executable.sensor;
                            let target = require_target(conn, authority, RuntimeTarget::Component(ComponentTarget::Sensor, sensor.id), &[Action::Update, Action::Decrypt]).await?;
                            if target.context().target_ref.as_deref() != Some(sensor.r#ref.as_str())
                                || target.context().pack_ref != sensor.pack_ref
                                || target.pack_id() != sensor.pack { return Err(denied()); }
                            let rule = if entity_type == ENTITY_SENSOR_GENERATION_RULE_CONFIG {
                                let id = attune_common::repositories::sensor_read_origin::rule_id_from_path(&path).ok_or_else(denied)?;
                                Some(sensor_frontier.initial_rule(generation, id).ok_or_else(denied)?)
                            } else { rule };
                            if let Some((id, reference, pack)) = rule {
                                let target = require_target(conn, authority, RuntimeTarget::Component(ComponentTarget::Rule, id), &[Action::Read, Action::Decrypt]).await?;
                                if !sensor_frontier.rule_owner_matches(&entity_type, entity_id, id, &reference, &pack, generation, &binding, &target)? { return Err(denied()); }
                            }
                        }
                        attune_common::secret_values::ENTITY_INQUIRY_OPTIONS
                        | attune_common::secret_values::ENTITY_INQUIRY_RESPONSE => {
                            use attune_common::repositories::{inquiry::InquiryRepository, inquiry_schema::InquirySchemaRepository};
                            // Explicit typed read/decrypt, never the human
                            // participant shortcut or the sponsor's current IAM.
                            let target = require_target(conn, authority, RuntimeTarget::Inquiry(entity_id), &[Action::Read, Action::Decrypt]).await?;
                            let inquiry = InquiryRepository::find_by_id(&mut *conn, entity_id).await?.ok_or_else(denied)?;
                            let schema = InquirySchemaRepository::find(&mut *conn, entity_id).await?.ok_or_else(denied)?;
                            let creator_target = require_target(conn, authority, RuntimeTarget::Execution(inquiry.created_by_execution), &[Action::Read, Action::Decrypt]).await?;
                            let creator = attune_common::repositories::execution::ExecutionRepository::find_by_id(&mut *conn, inquiry.created_by_execution).await?.ok_or_else(denied)?;
                            let snapshot = creator.executable_snapshot.as_ref().ok_or_else(denied)?;
                            if schema.binding.contract.source.component_ref() != creator.action_ref
                                || snapshot.executable.action.r#ref != creator.action_ref
                                || target.pack_id() != creator_target.pack_id()
                                || creator_target.pack_id() != Some(snapshot.executable.action.pack) { return Err(denied()); }
                        }
                        _ => return Err(denied_at(OriginDenialStage::UnknownEntity)),
                    }
                    // The current source target/schema checks above remain
                    // mandatory. Only A's authenticated direct public/native
                    // leaves may stop without a secret parent row. Never carry
                    // this exception into a generic read's saved parent graph.
                    if leaves.as_ref().is_some_and(|leaves| {
                        leaves.is_public_source(&entity_type, entity_id, &path)
                            || leaves.is_outcome_metadata(&entity_type, entity_id, &path)
                    }) {
                        continue;
                    }
                    let parent = if let Some(parent) = immutable_parent {
                        if retained
                            .as_ref()
                            .and_then(|rows| {
                                rows.get(&(entity_type.clone(), entity_id, path.clone()))
                            })
                            .is_some_and(|stored| stored != &parent)
                        {
                            return Err(denied_at(OriginDenialStage::FrozenParentConflict));
                        }
                        parent
                    } else if let Some(retained) = &retained {
                        retained
                            .get(&(entity_type.clone(), entity_id, path.clone()))
                            .cloned()
                            .ok_or_else(|| denied_at(OriginDenialStage::FrozenParentMissing))?
                    } else {
                        let parents = ExecutionSecretValueRepository::find_bounded_provenance(
                            &mut *conn,
                            &[entity_type.as_str()],
                            &[entity_id],
                        )
                        .await?;
                        let parent = parents
                            .into_iter()
                            .find(|row| row.json_path == path)
                            .ok_or_else(denied)?;
                        if parent.source_kind != "provenance" {
                            return Err(denied());
                        }
                        serde_json::from_str::<SecretProvenance>(
                            parent.source_ref.as_deref().ok_or_else(denied)?,
                        )
                        .map_err(|_| denied())?
                    };
                    let key = (entity_type.clone(), entity_id, path.clone());
                    if let Some(previous) = captured_parents.get(&key) {
                        // Distinct immutable reads can legitimately remember
                        // different revisions of one mutable source. They stay
                        // isolated in their own registry node, never overwrite
                        // a request-global parent with a wider current value.
                        if previous != &parent && retained.is_none() {
                            return Err(denied());
                        }
                    } else if retained.is_none() {
                        captured_parents.insert(key, parent.clone());
                    }
                    let site =
                        attune_common::repositories::immutable_input_origin::input_value_site(
                            &entity_type,
                            entity_id,
                            &path,
                        )
                        .ok();
                    pending.push((
                        "provenance".into(),
                        Some(serde_json::to_string(&parent).map_err(|_| denied())?),
                        depth + 1,
                        ancestry,
                        retained.clone(),
                        site,
                        None,
                    ));
                    if pending.len() + *nodes > MAX_ORIGIN_NODES {
                        return Err(denied());
                    }
                }
            }
        }
    }
    // Publish the closure only after every pending reader check succeeds.
    // F consumes this same budget; no handler fabricates receipt-ID parents.
    for (key, parent) in receipt_parents {
        if let Some(saved) = captured_parents.get(&key) {
            if saved != &parent {
                return Err(denied_at(OriginDenialStage::ReceiptParentConflict));
            }
        } else {
            if captured_parents.len() >= MAX_ORIGIN_NODES {
                return Err(denied());
            }
            captured_parents.insert(key, parent);
        }
    }
    receipt_frontier.ensure_fresh_on(conn).await?;
    authority.ensure_fresh_on(conn).await?;
    Ok(())
}

#[cfg(test)]
mod receipt_snapshot_tests {
    use super::*;

    #[test]
    fn internal_denial_stages_never_change_the_public_error() {
        for stage in [
            OriginDenialStage::PrepareSources,
            OriginDenialStage::CaptureRows,
            OriginDenialStage::TargetPermission,
            OriginDenialStage::ReadExecutionTarget,
            OriginDenialStage::ReadArtifactTarget,
            OriginDenialStage::ReadVersionPending,
            OriginDenialStage::ReceiptContext,
            OriginDenialStage::ReceiptDescriptor,
            OriginDenialStage::FrozenParentMissing,
            OriginDenialStage::LocalOrigin,
            OriginDenialStage::NativeSuccessReader,
        ] {
            let ApiError::Forbidden(message) = denied_at(stage) else {
                panic!("Wrong error class");
            };
            assert_eq!(message, "Recorded secret-origin disclosure is unavailable");
        }
    }

    #[test]
    fn frozen_receipt_terminals_are_not_local_authority() {
        let ancestry = BTreeSet::new();
        for terminal in [
            "receipt_literal",
            "receipt_public_source",
            "receipt_outcome_metadata",
        ] {
            assert!(!local_origin_allowed(terminal, &ancestry));
        }
    }

    #[test]
    fn receipt_snapshots_retain_the_original_typed_delivery_site() {
        for kind in ["execution_config", "execution_result", "workflow_variables"] {
            let receipt = SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: format!("declared_input:{kind}:value"),
                    entity_id: 91,
                    path: "/token".into(),
                }],
                templates: vec![],
            };
            let row = SecretProvenanceRow {
                entity_type: kind.into(),
                entity_id: 42,
                json_path: "/token".into(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&receipt).unwrap()),
            };
            assert_eq!(
                snapshot(&[row]).unwrap(),
                vec![SecretProvenance {
                    origins: vec![SecretOrigin::Entity {
                        entity_type: kind.into(),
                        entity_id: 42,
                        path: "/token".into(),
                    }],
                    templates: vec![],
                }]
            );
        }
    }

    #[test]
    fn receipt_without_an_original_delivery_site_cannot_be_snapshotted() {
        let row = SecretProvenanceRow {
            entity_type: "artifact_file_body".into(),
            entity_id: 42,
            json_path: "/token".into(),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(&SecretProvenance {
                    origins: vec![SecretOrigin::Entity {
                        entity_type: "declared_input:execution_result:value".into(),
                        entity_id: 91,
                        path: "/token".into(),
                    }],
                    templates: vec![],
                })
                .unwrap(),
            ),
        };
        assert!(snapshot(&[row]).is_err());
    }
}

#[cfg(test)]
pub(crate) mod success_tests;

#[expect(
    clippy::too_many_arguments,
    reason = "Disclosure checks the exact source, origin, traversal budget, and actor independently"
)]
async fn authorize_success_source(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    source: &attune_common::repositories::native_input_disclosure::PreparedSuccessSource,
    retained: Option<&std::collections::BTreeMap<(String, i64, String), SecretProvenance>>,
    parents: &mut std::collections::BTreeMap<(String, i64, String), SecretProvenance>,
    charged: &mut BTreeSet<i64>,
    nodes: &mut usize,
    bytes: &mut usize,
) -> ApiResult<()> {
    if charged.insert(source.metadata().snapshot()) {
        *nodes = nodes
            .checked_add(source.retained_nodes())
            .ok_or_else(denied)?;
        *bytes = bytes
            .checked_add(source.retained_bytes())
            .ok_or_else(denied)?;
        if *nodes > MAX_ORIGIN_NODES || *bytes > MAX_ORIGIN_BYTES {
            return Err(denied_at(OriginDenialStage::Bounds));
        }
    }
    for target in RuntimeTargetRepository::resolve_native_success_source(
        conn,
        source,
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    {
        if [Action::Read, Action::Decrypt]
            .into_iter()
            .any(|action| !authority.allows_target(&target, action))
        {
            return Err(denied_at(OriginDenialStage::NativeSuccessReader));
        }
    }
    for parent in source.parents() {
        let key = (
            parent.entity_type.clone(),
            parent.entity_id,
            parent.path.clone(),
        );
        if let Some(retained) = retained {
            if retained.get(&key) != Some(&parent.provenance) {
                return Err(denied_at(OriginDenialStage::FrozenParentConflict));
            }
        } else if let Some(saved) = parents.get(&key) {
            if saved != &parent.provenance {
                return Err(denied_at(OriginDenialStage::FrozenParentConflict));
            }
        } else {
            if parents.len() >= MAX_ORIGIN_NODES {
                return Err(denied_at(OriginDenialStage::Bounds));
            }
            parents.insert(key, parent.provenance.clone());
        }
    }
    authority.ensure_fresh_on(conn).await?;
    source.ensure_fresh()?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Wait disclosure keeps source identity, origin, traversal budget, and actor checks explicit"
)]
async fn authorize_wait_completion_source(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    source: &attune_common::repositories::native_input_disclosure::PreparedWaitCompletionSource,
    retained: Option<&std::collections::BTreeMap<(String, i64, String), SecretProvenance>>,
    parents: &mut std::collections::BTreeMap<(String, i64, String), SecretProvenance>,
    charged: &mut BTreeSet<i64>,
    nodes: &mut usize,
    bytes: &mut usize,
) -> ApiResult<()> {
    if charged.insert(source.metadata().snapshot()) {
        *nodes = nodes
            .checked_add(source.retained_nodes())
            .ok_or_else(denied)?;
        *bytes = bytes
            .checked_add(source.retained_bytes())
            .ok_or_else(denied)?;
        if *nodes > MAX_ORIGIN_NODES || *bytes > MAX_ORIGIN_BYTES {
            return Err(denied_at(OriginDenialStage::Bounds));
        }
    }
    for target in RuntimeTargetRepository::resolve_native_wait_completion_source(
        conn,
        source,
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    {
        if [Action::Read, Action::Decrypt]
            .into_iter()
            .any(|action| !authority.allows_target(&target, action))
        {
            return Err(denied_at(OriginDenialStage::NativeWaitCompletionReader));
        }
    }
    for parent in source.parents() {
        let key = (
            parent.entity_type.clone(),
            parent.entity_id,
            parent.path.clone(),
        );
        if let Some(retained) = retained {
            if retained.get(&key) != Some(&parent.provenance) {
                return Err(denied_at(OriginDenialStage::FrozenParentConflict));
            }
        } else if let Some(saved) = parents.get(&key) {
            if saved != &parent.provenance {
                return Err(denied_at(OriginDenialStage::FrozenParentConflict));
            }
        } else {
            if parents.len() >= MAX_ORIGIN_NODES {
                return Err(denied_at(OriginDenialStage::Bounds));
            }
            parents.insert(key, parent.provenance.clone());
        }
    }
    authority.ensure_fresh_on(conn).await?;
    source.ensure_fresh()?;
    Ok(())
}
