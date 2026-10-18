//! Request-local source records, not recursive authorization decisions.
//! Frontier discovery is independent of each evaluator's active path/depth.

use std::collections::{BTreeMap, HashMap, HashSet};

use attune_common::{
    event_schema::EventSchemaBinding,
    rbac::Grant,
    repositories::{
        component_configuration::{
            ComponentConfigurationRepository, ConfigurationDisclosureRow,
            ENTITY_COMPONENT_CONFIGURATION,
        },
        event::{EventDisclosureRow, EventRepository},
        event_schema::EventSchemaRepository,
        execution::{ExecutionAuthorizationRow, ExecutionRepository},
        execution_secret_value::{ExecutionSecretValueRepository, SecretProvenanceRow},
        inquiry::{InquiryDisclosureRow, InquiryRepository},
        inquiry_schema::InquirySchemaRepository,
        rule::RuleRepository,
        sensor_schema::{
            SensorDeliveryDisclosureRow, SensorDisclosureRow, SensorSchemaRepository,
            ENTITY_SENSOR_GENERATION_CONFIG, ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        },
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{
        ENTITY_EVENT_PAYLOAD, ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT,
        ENTITY_INQUIRY_OPTIONS, ENTITY_INQUIRY_RESPONSE,
    },
};

use crate::{
    auth::{jwt::TokenType, middleware::AuthenticatedUser},
    middleware::ApiResult,
    routes::executions::ExecutionVisibilityCache,
};

const SOURCE_BATCH_SIZE: usize = 64;

#[cfg(test)]
mod frozen_parent_tests {
    use super::*;

    #[test]
    fn discovery_includes_workflow_variable_receipt_parents_without_native_aliases() {
        for kind in [
            ENTITY_EXECUTION_CONFIG,
            ENTITY_EXECUTION_RESULT,
            "workflow_variables",
        ] {
            assert!(supported_entity(kind));
        }
        for kind in [
            "workflow_variable",
            "declared_input:workflow_variables:value",
            "execution_result_view",
            "workflow_failure_metadata",
        ] {
            assert!(!supported_entity(kind));
        }
    }

    #[test]
    fn typed_expansion_keeps_exact_read_parents_without_current_row_repair() {
        let saved = attune_common::repositories::runtime_read::RuntimeReadParent {
            entity_type: ENTITY_EXECUTION_RESULT.into(),
            entity_id: 11,
            path: String::new(),
            provenance: SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: "receipt_public_source".into(),
                    source_ref: None,
                }],
                templates: vec![],
            },
        };
        let mut outer = DisclosureRecords::default();
        outer
            .retain_frozen_parents(std::slice::from_ref(&saved))
            .unwrap();
        let mut child = DisclosureRecords::default();
        child.seed_execution_provenance(
            &[11],
            [SecretProvenanceRow {
                entity_type: ENTITY_EXECUTION_RESULT.into(),
                entity_id: 11,
                json_path: "/current-only".into(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&saved.provenance).unwrap()),
            }],
        );
        child.inherit_frozen_parents(&outer).unwrap();
        assert!(std::sync::Arc::ptr_eq(
            outer.frozen_parents.as_ref().unwrap(),
            child.frozen_parents.as_ref().unwrap()
        ));
        assert!(child.frozen_parent_matches(&saved));
        assert!(child
            .parent(ENTITY_EXECUTION_RESULT, 11, "/current-only")
            .is_none());
        let parent = child.parent(ENTITY_EXECUTION_RESULT, 11, "").unwrap();
        assert_eq!(
            serde_json::from_str::<SecretProvenance>(parent.source_ref.as_deref().unwrap())
                .unwrap(),
            saved.provenance
        );
        let mut wrong = saved;
        wrong.path = "/wrong".into();
        assert!(!child.frozen_parent_matches(&wrong));
    }
}

type FrozenParentOrigins =
    std::sync::Arc<std::collections::BTreeMap<(String, i64, String), SecretProvenance>>;

#[derive(Default)]
pub(crate) struct DisclosureRecords {
    frozen_parents: Option<FrozenParentOrigins>,
    receipt_freshness: attune_common::repositories::immutable_input_origin::ReceiptFreshness,
    pub(crate) exact_events:
        HashMap<i64, attune_common::repositories::event_origin::EventOriginEvidence>,
    pub(crate) exact_event_scopes:
        HashMap<i64, attune_common::repositories::event_origin::VerifiedEventSourceScope>,
    exact_events_seen: HashSet<i64>,
    exact_event_bytes: usize,
    pub(crate) runtime_reads:
        HashMap<i64, attune_common::repositories::runtime_read::RuntimeReadOrigin>,
    runtime_read_bytes: usize,
    runtime_read_nodes: usize,
    sensor_sources: HashSet<(String, i64, String)>,
    pub(crate) sensor_caller: Option<attune_common::rbac::AuthorizationContext>,
    pub(crate) sensor_rule_proofs: crate::sensor_origin_iam::SensorRuleProofCache,
    pub(crate) key_ownership: attune_common::repositories::iam_ownership::VerifiedKeyOrigins,
    key_ownership_loaded: HashSet<i64>,
    pub(crate) queue_origin_packs: HashMap<(i64, String, String), i64>,
    queue_origins_loaded: HashSet<i64>,
    provenance: HashMap<(String, i64), HashMap<String, SecretProvenanceRow>>,
    executions: HashMap<i64, Option<ExecutionAuthorizationRow>>,
    pub(crate) visibility: ExecutionVisibilityCache,
    pub(crate) components: HashMap<i64, Option<ConfigurationDisclosureRow>>,
    pub(crate) events: HashMap<i64, Option<EventDisclosureRow>>,
    pub(crate) event_bindings: HashMap<i64, EventSchemaBinding>,
    pub(crate) event_visibility: HashMap<(i64, bool), bool>,
    pub(crate) inquiries: HashMap<i64, Option<InquiryDisclosureRow>>,
    pub(crate) inquiry_bindings: HashSet<i64>,
    pub(crate) inquiry_caller: Option<attune_common::rbac::AuthorizationContext>,
    pub(crate) generations: HashMap<i64, Option<SensorDisclosureRow>>,
    pub(crate) deliveries: HashMap<i64, Option<SensorDeliveryDisclosureRow>>,
    pub(crate) rules: HashMap<i64, Option<String>>,
    pub(crate) rule_pack_ids: HashMap<i64, i64>,
}

impl DisclosureRecords {
    pub(crate) async fn load_sensor_sources(
        &mut self,
        db: &sqlx::PgPool,
        user: &AuthenticatedUser,
        grants: &[Grant],
        sources: &HashSet<(String, i64, String)>,
    ) -> ApiResult<()> {
        self.load_sources(db, user, grants, sources).await
    }

    pub(crate) fn frozen_parent_matches(
        &self,
        parent: &attune_common::repositories::runtime_read::RuntimeReadParent,
    ) -> bool {
        self.frozen_parents.as_ref().is_none_or(|parents| {
            parents.get(&(
                parent.entity_type.clone(),
                parent.entity_id,
                parent.path.clone(),
            )) == Some(&parent.provenance)
        })
    }

    pub(crate) fn inherit_frozen_parents(&mut self, other: &Self) -> ApiResult<()> {
        self.frozen_parents = other.frozen_parents.clone();
        self.seed_frozen_parents()
    }

    pub(crate) fn retain_frozen_parents(
        &mut self,
        parents: &[attune_common::repositories::runtime_read::RuntimeReadParent],
    ) -> ApiResult<()> {
        self.frozen_parents = Some(std::sync::Arc::new(
            parents
                .iter()
                .map(|parent| {
                    (
                        (
                            parent.entity_type.clone(),
                            parent.entity_id,
                            parent.path.clone(),
                        ),
                        parent.provenance.clone(),
                    )
                })
                .collect(),
        ));
        self.seed_frozen_parents()
    }

    fn seed_frozen_parents(&mut self) -> ApiResult<()> {
        let Some(parents) = &self.frozen_parents else {
            return Ok(());
        };
        let ids = parents
            .keys()
            .filter(|(kind, _, _)| {
                matches!(
                    kind.as_str(),
                    ENTITY_EXECUTION_CONFIG | ENTITY_EXECUTION_RESULT | "workflow_variables"
                )
            })
            .map(|(_, id, _)| *id)
            .collect::<Vec<_>>();
        for (kind, id, _) in parents.keys() {
            self.provenance.entry((kind.clone(), *id)).or_default();
        }
        // Mark identities as frozen without cloning the complete graph at each
        // typed receipt expansion. parent() serves only the requested saved row.
        self.seed_execution_provenance(&ids, std::iter::empty());
        Ok(())
    }

    pub(crate) fn retain_receipt_freshness(
        &mut self,
        freshness: attune_common::repositories::immutable_input_origin::ReceiptFreshness,
    ) -> ApiResult<()> {
        self.receipt_freshness.append(freshness)?;
        Ok(())
    }

    pub(crate) fn take_receipt_freshness(
        &mut self,
    ) -> attune_common::repositories::immutable_input_origin::ReceiptFreshness {
        std::mem::take(&mut self.receipt_freshness)
    }
    pub(crate) fn seed_execution_provenance(
        &mut self,
        ids: &[i64],
        rows: impl IntoIterator<Item = SecretProvenanceRow>,
    ) {
        for id in ids {
            for kind in [
                ENTITY_EXECUTION_CONFIG,
                ENTITY_EXECUTION_RESULT,
                "workflow_variables",
            ] {
                self.provenance.entry((kind.into(), *id)).or_default();
            }
        }
        for row in rows {
            self.provenance
                .entry((row.entity_type.clone(), row.entity_id))
                .or_default()
                .insert(row.json_path.clone(), row);
        }
    }

    pub(crate) fn seed_executions(
        &mut self,
        rows: impl IntoIterator<Item = ExecutionAuthorizationRow>,
    ) {
        for row in rows {
            self.executions.insert(row.id, Some(row));
        }
    }

    pub(crate) async fn prepare<'a>(
        &mut self,
        db: &sqlx::PgPool,
        user: &AuthenticatedUser,
        grants: &[Grant],
        secrets: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    ) -> ApiResult<()> {
        let mut frontier = secrets
            .into_iter()
            .map(|(kind, reference)| (kind.to_string(), reference.map(str::to_string)))
            .collect::<Vec<_>>();
        let mut expanded = HashSet::new();
        // At evaluator depth 31 an entity origin still checks its source scope
        // before the recursive call rejects depth 32. Fetch that final frontier,
        // but never expand it. A shared node's shallowest path discovers all
        // records needed; this does not decide whether a deeper traversal allows.
        for _ in 0..32 {
            let mut sources = HashSet::new();
            let mut keys = HashSet::new();
            let mut queues = HashSet::new();
            for (source_kind, source_ref) in &frontier {
                if source_kind != "provenance" {
                    continue;
                }
                let Some(provenance) = source_ref
                    .as_deref()
                    .and_then(|reference| serde_json::from_str::<SecretProvenance>(reference).ok())
                else {
                    continue;
                };
                for origin in provenance.origins {
                    if let SecretOrigin::QueueItem { queue_id, .. } = &origin {
                        if !self.queue_origins_loaded.contains(queue_id) {
                            queues.insert(*queue_id);
                        }
                    }
                    if let SecretOrigin::Key(ref key) = origin {
                        if !self.key_ownership_loaded.contains(&key.key_id) {
                            keys.insert(key.key_id);
                        }
                    }
                    if let SecretOrigin::Entity {
                        entity_type,
                        entity_id,
                        path,
                    } = origin
                    {
                        if supported_entity(&entity_type)
                            && expanded.insert((entity_type.clone(), entity_id, path.clone()))
                        {
                            sources.insert((entity_type, entity_id, path));
                        }
                    }
                }
            }
            if grants.iter().any(|grant| {
                grant.resource == attune_common::rbac::Resource::Keys
                    && grant
                        .constraints
                        .as_ref()
                        .is_some_and(|scope| scope.managing_pack_id.is_some())
            }) {
                let keys = keys.into_iter().collect::<Vec<_>>();
                for batch in keys.chunks(SOURCE_BATCH_SIZE) {
                    self.key_ownership.extend(
                        attune_common::repositories::iam_ownership::VerifiedKeyOrigins::load(
                            db, batch,
                        )
                        .await?,
                    );
                    self.key_ownership_loaded.extend(batch.iter().copied());
                }
            }
            if grants.iter().any(|grant| {
                grant.resource == attune_common::rbac::Resource::Queues
                    && grant
                        .constraints
                        .as_ref()
                        .is_some_and(|scope| scope.managing_pack_id.is_some())
            }) {
                let queues = queues.into_iter().collect::<Vec<_>>();
                for batch in queues.chunks(SOURCE_BATCH_SIZE) {
                    self.queue_origin_packs.extend(attune_common::repositories::iam_ownership::IamOwnershipRepository::queue_origin_packs(db, batch).await?);
                    self.queue_origins_loaded.extend(batch.iter().copied());
                }
            }
            if sources.is_empty() {
                break;
            }
            let event_sources = sources
                .iter()
                .filter(|(kind, id, _)| {
                    matches!(
                        kind.as_str(),
                        attune_common::repositories::event_origin::EVENT_SOURCE_PAYLOAD
                            | attune_common::repositories::event_origin::EVENT_SOURCE_CONFIG
                    ) && !self.exact_events_seen.contains(id)
                })
                .map(|(_, id, _)| *id)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if self.exact_events_seen.len() + event_sources.len() > 256 {
                return Err(crate::middleware::ApiError::Forbidden(
                    "Event origin graph exceeds its bound".into(),
                ));
            }
            for batch in event_sources.chunks(SOURCE_BATCH_SIZE) {
                use attune_common::repositories::event_origin::EventOriginRepository;
                let filters = [
                    attune_common::rbac::Action::Read,
                    attune_common::rbac::Action::Decrypt,
                ]
                .map(|action| {
                    crate::routes::events::event_action_visibility_filter(user, grants, action)
                });
                if filters.iter().any(Result::is_err) {
                    // Missing source authority must not expose corrupt retained
                    // schema/provenance through a different error response.
                    self.exact_events_seen.extend(batch);
                    continue;
                }
                let evidence = EventOriginRepository::evidence_many(db, batch).await?;
                if !evidence.is_empty() {
                    let mut connection = db.acquire().await?;
                    let identity = user.identity_id().map_err(|_| {
                        crate::middleware::ApiError::Forbidden(
                            "Event source identity is unavailable".into(),
                        )
                    })?;
                    let mut scopes =
                        EventOriginRepository::human_scopes(&mut connection, &evidence, identity)
                            .await?;
                    let identities = evidence
                        .iter()
                        .map(|(_, source)| (source.event, source.created))
                        .collect::<Vec<_>>();
                    for filter in filters {
                        if let Ok(Some(filter)) = filter {
                            let visible = EventRepository::visible_identities(
                                &mut *connection,
                                &identities,
                                &filter,
                            )
                            .await?
                            .into_iter()
                            .collect::<HashSet<_>>();
                            scopes.retain(|id, _| {
                                evidence.iter().any(|(source, row)| {
                                    source == id && visible.contains(&(row.event, row.created))
                                })
                            });
                        }
                    }
                    self.exact_event_scopes.extend(scopes);
                }
                self.exact_events_seen.extend(batch);
                for (id, source) in evidence {
                    self.exact_event_bytes += serde_json::to_vec(&source)
                        .map_err(|_| {
                            crate::middleware::ApiError::Forbidden(
                                "Event source is unavailable".into(),
                            )
                        })?
                        .len();
                    if self.exact_event_bytes > 1024 * 1024 {
                        return Err(crate::middleware::ApiError::Forbidden(
                            "Event origin bytes exceed their bound".into(),
                        ));
                    }
                    self.exact_events.insert(id, source);
                }
            }
            for (kind, id, path) in &sources {
                use attune_common::repositories::event_origin::{
                    EVENT_SOURCE_CONFIG, EVENT_SOURCE_PAYLOAD,
                };
                if matches!(kind.as_str(), EVENT_SOURCE_PAYLOAD | EVENT_SOURCE_CONFIG) {
                    let Some(evidence) = self.exact_events.get(id) else {
                        continue;
                    };
                    let parent = evidence.parent(kind, path)?;
                    let reference = serde_json::to_string(parent).map_err(|_| {
                        crate::middleware::ApiError::Forbidden("Event source is unavailable".into())
                    })?;
                    self.provenance
                        .entry((kind.clone(), *id))
                        .or_default()
                        .insert(
                            path.clone(),
                            SecretProvenanceRow {
                                entity_type: kind.clone(),
                                entity_id: *id,
                                json_path: path.clone(),
                                source_kind: "provenance".into(),
                                source_ref: Some(reference),
                            },
                        );
                }
                use attune_common::repositories::runtime_read::{
                    RuntimeReadRepository, RuntimeReadTarget, ENTITY_RUNTIME_READ,
                };
                if kind != ENTITY_RUNTIME_READ || self.runtime_reads.contains_key(id) {
                    continue;
                }
                if !path.is_empty() || self.runtime_reads.len() >= 256 {
                    return Err(crate::middleware::ApiError::Forbidden(
                        "Runtime read disclosure exceeds its bound".into(),
                    ));
                }
                let read = RuntimeReadRepository::find(db, *id).await?;
                let bytes = serde_json::to_vec(&read)
                    .map_err(|_| {
                        crate::middleware::ApiError::Forbidden(
                            "Runtime read disclosure is unavailable".into(),
                        )
                    })?
                    .len();
                self.runtime_read_bytes =
                    self.runtime_read_bytes.checked_add(bytes).ok_or_else(|| {
                        crate::middleware::ApiError::Forbidden(
                            "Runtime read disclosure exceeds its bound".into(),
                        )
                    })?;
                self.runtime_read_nodes += 1 + read
                    .provenance
                    .iter()
                    .map(|parent| parent.origins.len())
                    .sum::<usize>();
                if self.runtime_read_bytes > 1024 * 1024 || self.runtime_read_nodes > 256 {
                    return Err(crate::middleware::ApiError::Forbidden(
                        "Runtime read disclosure exceeds its bound".into(),
                    ));
                }
                let execution_ids = match &read.target {
                    RuntimeReadTarget::Execution { execution }
                    | RuntimeReadTarget::ExecutionMetadata { execution }
                    | RuntimeReadTarget::LiveLog { execution, .. } => vec![*execution],
                    RuntimeReadTarget::ArtifactVersion { execution, .. } => {
                        execution.iter().copied().collect()
                    }
                    RuntimeReadTarget::Artifact { executions, .. } => executions.clone(),
                };
                self.runtime_read_nodes += execution_ids.len();
                if self.runtime_read_nodes > 256 {
                    return Err(crate::middleware::ApiError::Forbidden(
                        "Runtime read disclosure exceeds its bound".into(),
                    ));
                }
                self.load_executions(db, &execution_ids).await?;
                let mut provenance = SecretProvenance::default();
                for parent in &read.provenance {
                    provenance.merge(parent.clone());
                }
                let reference = serde_json::to_string(&provenance).map_err(|_| {
                    crate::middleware::ApiError::Forbidden(
                        "Runtime read disclosure is unavailable".into(),
                    )
                })?;
                if reference.len() > 1024 * 1024 || provenance.origins.len() > 256 {
                    return Err(crate::middleware::ApiError::Forbidden(
                        "Runtime read disclosure exceeds its bound".into(),
                    ));
                }
                self.provenance.insert(
                    (kind.clone(), *id),
                    HashMap::from([(
                        String::new(),
                        SecretProvenanceRow {
                            entity_type: kind.clone(),
                            entity_id: *id,
                            json_path: String::new(),
                            source_kind: "provenance".into(),
                            source_ref: Some(reference),
                        },
                    )]),
                );
                self.runtime_reads.insert(*id, read);
            }
            let mut by_kind = BTreeMap::<String, Vec<i64>>::new();
            let mut execution_ids = Vec::new();
            for (kind, id, _) in &sources {
                if !self.provenance.contains_key(&(kind.clone(), *id)) {
                    by_kind.entry(kind.clone()).or_default().push(*id);
                }
                if matches!(
                    kind.as_str(),
                    ENTITY_EXECUTION_CONFIG | ENTITY_EXECUTION_RESULT | "workflow_variables"
                ) {
                    execution_ids.push(*id);
                }
            }
            self.load_execution_provenance(db, &execution_ids).await?;
            for (kind, mut ids) in by_kind {
                if matches!(
                    kind.as_str(),
                    ENTITY_EXECUTION_CONFIG | ENTITY_EXECUTION_RESULT | "workflow_variables"
                ) {
                    continue;
                }
                ids.sort_unstable();
                ids.dedup();
                for batch in ids.chunks(SOURCE_BATCH_SIZE) {
                    let rows = ExecutionSecretValueRepository::find_provenance_by_entities(
                        db,
                        &[kind.as_str()],
                        batch,
                    )
                    .await?;
                    for id in batch {
                        self.provenance.insert((kind.clone(), *id), HashMap::new());
                    }
                    for row in rows {
                        self.provenance
                            .entry((row.entity_type.clone(), row.entity_id))
                            .or_default()
                            .insert(row.json_path.clone(), row);
                    }
                }
            }
            self.load_executions(db, &execution_ids).await?;
            self.load_sources(db, user, grants, &sources).await?;
            frontier = sources
                .into_iter()
                .filter(|(kind, _, _)| {
                    kind != attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ
                })
                .filter_map(|(kind, id, path)| self.parent(&kind, id, &path))
                .map(|row| (row.source_kind, row.source_ref))
                .collect();
        }
        if !self.sensor_sources.is_empty() && self.sensor_caller.is_none() {
            use attune_common::repositories::{identity::IdentityRepository, FindById};
            let identity = user.identity_id().map_err(|_| {
                crate::middleware::ApiError::Unauthorized("Invalid disclosure identity".into())
            })?;
            let row = IdentityRepository::find_by_id(db, identity)
                .await?
                .ok_or_else(|| {
                    crate::middleware::ApiError::Unauthorized(
                        "Disclosure identity is unavailable".into(),
                    )
                })?;
            let mut caller = attune_common::rbac::AuthorizationContext::new(identity);
            caller.identity_attributes = match row.attributes {
                serde_json::Value::Object(attributes) => attributes.into_iter().collect(),
                _ => Default::default(),
            };
            self.sensor_caller = Some(caller);
        }
        self.load_sensor_rule_proofs(db, &self.sensor_sources.clone())
            .await?;
        if grants.iter().any(|grant| {
            grant.resource == attune_common::rbac::Resource::Executions
                && grant
                    .constraints
                    .as_ref()
                    .is_some_and(|scope| scope.managing_pack_id.is_some())
        }) {
            let ids = self.executions.keys().copied().collect::<Vec<_>>();
            self.visibility.load_ownership(db, &ids).await?;
        }
        Ok(())
    }

    async fn load_execution_provenance(&mut self, db: &sqlx::PgPool, ids: &[i64]) -> ApiResult<()> {
        let mut missing = ids
            .iter()
            .copied()
            .filter(|id| {
                [
                    ENTITY_EXECUTION_CONFIG,
                    ENTITY_EXECUTION_RESULT,
                    "workflow_variables",
                ]
                .iter()
                .any(|kind| !self.provenance.contains_key(&(kind.to_string(), *id)))
            })
            .collect::<Vec<_>>();
        missing.sort_unstable();
        missing.dedup();
        for batch in missing.chunks(SOURCE_BATCH_SIZE) {
            let rows = ExecutionSecretValueRepository::find_provenance_by_entities(
                db,
                &[
                    ENTITY_EXECUTION_CONFIG,
                    ENTITY_EXECUTION_RESULT,
                    "workflow_variables",
                ],
                batch,
            )
            .await?;
            self.seed_execution_provenance(batch, rows);
        }
        Ok(())
    }

    pub(crate) fn parent(&self, kind: &str, id: i64, path: &str) -> Option<SecretProvenanceRow> {
        if let Some(parents) = &self.frozen_parents {
            let provenance = parents.get(&(kind.to_owned(), id, path.to_owned()))?;
            return Some(SecretProvenanceRow {
                entity_type: kind.to_owned(),
                entity_id: id,
                json_path: path.to_owned(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(provenance).ok()?),
            });
        }
        self.provenance
            .get(&(kind.to_string(), id))
            .and_then(|rows| rows.get(path))
            .cloned()
    }

    pub(crate) fn execution(&self, id: i64) -> Option<ExecutionAuthorizationRow> {
        self.executions.get(&id).and_then(Option::as_ref).cloned()
    }

    pub(crate) async fn load_executions(
        &mut self,
        db: &sqlx::PgPool,
        ids: &[i64],
    ) -> ApiResult<()> {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let missing = ids
            .iter()
            .copied()
            .filter(|id| !self.executions.contains_key(id))
            .collect::<Vec<_>>();
        for batch in missing.chunks(SOURCE_BATCH_SIZE) {
            let rows = ExecutionRepository::find_authorization_rows(db, batch).await?;
            for id in batch {
                self.executions.insert(*id, None);
            }
            self.seed_executions(rows);
        }
        let refs = ids
            .iter()
            .filter_map(|id| self.executions.get(id).and_then(Option::as_ref))
            .filter(|row| row.parent.is_none())
            .map(|row| row.action_ref.clone())
            .collect::<Vec<_>>();
        self.visibility.load_public_actions(db, &refs).await?;
        let children = ids
            .into_iter()
            .filter(|id| {
                self.executions
                    .get(id)
                    .and_then(Option::as_ref)
                    .is_some_and(|row| row.parent.is_some())
            })
            .collect::<Vec<_>>();
        self.visibility.load(db, &children).await?;
        Ok(())
    }

    async fn load_sources(
        &mut self,
        db: &sqlx::PgPool,
        user: &AuthenticatedUser,
        grants: &[Grant],
        sources: &HashSet<(String, i64, String)>,
    ) -> ApiResult<()> {
        self.sensor_sources.extend(
            sources
                .iter()
                .filter(|(kind, _, _)| {
                    matches!(
                        kind.as_str(),
                        ENTITY_SENSOR_GENERATION_CONFIG
                            | ENTITY_SENSOR_GENERATION_RULE_CONFIG
                            | ENTITY_SENSOR_RULE_DELIVERY_CONFIG
                    )
                })
                .cloned(),
        );
        let mut kinds = BTreeMap::<&str, Vec<i64>>::new();
        for (kind, id, _) in sources {
            kinds.entry(kind).or_default().push(*id);
        }
        for (kind, mut ids) in kinds {
            ids.sort_unstable();
            ids.dedup();
            for batch in ids.chunks(SOURCE_BATCH_SIZE) {
                match kind {
                    ENTITY_COMPONENT_CONFIGURATION => {
                        let missing = batch
                            .iter()
                            .copied()
                            .filter(|id| !self.components.contains_key(id))
                            .collect::<Vec<_>>();
                        if !missing.is_empty() {
                            let rows = ComponentConfigurationRepository::find_disclosure_rows(
                                db, &missing,
                            )
                            .await?;
                            for id in missing {
                                self.components.insert(id, None);
                            }
                            for row in rows {
                                self.components.insert(row.id, Some(row));
                            }
                        }
                    }
                    ENTITY_EVENT_PAYLOAD => self.load_events(db, user, grants, batch).await?,
                    ENTITY_INQUIRY_OPTIONS | ENTITY_INQUIRY_RESPONSE => {
                        let missing = batch
                            .iter()
                            .copied()
                            .filter(|id| !self.inquiries.contains_key(id))
                            .collect::<Vec<_>>();
                        if !missing.is_empty() {
                            let rows =
                                InquiryRepository::find_disclosure_rows(db, &missing).await?;
                            if self.inquiry_caller.is_none() {
                                self.inquiry_caller = Some(
                                    crate::routes::inquiries::inquiry_authorization_caller(
                                        db, user,
                                    )
                                    .await?,
                                );
                            }
                            let creators = rows
                                .iter()
                                .map(|row| row.created_by_execution)
                                .collect::<Vec<_>>();
                            self.load_executions(db, &creators).await?;
                            if grants.iter().any(|grant| {
                                grant.resource == attune_common::rbac::Resource::Inquiries
                                    && grant
                                        .constraints
                                        .as_ref()
                                        .is_some_and(|scope| scope.managing_pack_id.is_some())
                            }) {
                                self.visibility.load_ownership(db, &creators).await?;
                            }
                            // The inquiry endpoint checks participant/read and
                            // decrypt scope before inspecting its binding. Keep
                            // corrupt bindings outside that scope undisclosed.
                            if let Some(caller) = self.inquiry_caller.as_ref() {
                                let ids = rows
                                    .iter()
                                    .filter(|row| {
                                        crate::routes::inquiries::inquiry_disclosure_scope_allows(
                                            row,
                                            self.executions
                                                .get(&row.created_by_execution)
                                                .and_then(Option::as_ref),
                                            self.visibility.managing_pack(row.created_by_execution),
                                            caller,
                                            grants,
                                        )
                                    })
                                    .map(|row| row.id)
                                    .collect::<Vec<_>>();
                                if !ids.is_empty() {
                                    for (id, _) in
                                        InquirySchemaRepository::find_many(db, &ids).await?
                                    {
                                        self.inquiry_bindings.insert(id);
                                    }
                                }
                            }
                            for id in missing {
                                self.inquiries.insert(id, None);
                            }
                            for row in rows {
                                self.inquiries.insert(row.id, Some(row));
                            }
                        }
                    }
                    ENTITY_SENSOR_RULE_DELIVERY_CONFIG => {
                        let missing = batch
                            .iter()
                            .copied()
                            .filter(|id| !self.deliveries.contains_key(id))
                            .collect::<Vec<_>>();
                        if !missing.is_empty() {
                            let rows =
                                SensorSchemaRepository::find_delivery_disclosure_rows(db, &missing)
                                    .await?;
                            for id in missing {
                                self.deliveries.insert(id, None);
                            }
                            for row in rows {
                                self.deliveries.insert(row.id, Some(row));
                            }
                        }
                        let retained = batch
                            .iter()
                            .filter_map(|id| self.deliveries.get(id).and_then(Option::as_ref))
                            .collect::<Vec<_>>();
                        let generations = retained
                            .iter()
                            .map(|row| row.generation_binding)
                            .collect::<Vec<_>>();
                        let rules = retained.iter().map(|row| row.rule).collect::<Vec<_>>();
                        self.load_generations(db, &generations).await?;
                        self.load_rules(db, &rules).await?;
                    }
                    ENTITY_SENSOR_GENERATION_CONFIG | ENTITY_SENSOR_GENERATION_RULE_CONFIG => {
                        self.load_generations(db, batch).await?
                    }
                    _ => {}
                }
            }
        }
        let mut rules = Vec::new();
        for (kind, id, path) in sources {
            if kind != ENTITY_SENSOR_GENERATION_RULE_CONFIG {
                continue;
            }
            if let Some(rule) = crate::sensor_origin_iam::generation_rule_id(path) {
                if self
                    .generations
                    .get(id)
                    .and_then(Option::as_ref)
                    .is_some_and(|row| row.initial_rules.get(rule.to_string()).is_some())
                {
                    rules.push(rule);
                }
            }
        }
        self.load_rules(db, &rules).await?;
        Ok(())
    }

    async fn load_sensor_rule_proofs(
        &mut self,
        db: &sqlx::PgPool,
        sources: &HashSet<(String, i64, String)>,
    ) -> ApiResult<()> {
        let mut requests = Vec::new();
        for (kind, id, path) in sources {
            use crate::sensor_origin_iam::RuleOriginSource;
            let source = if kind == ENTITY_SENSOR_GENERATION_RULE_CONFIG {
                self.generations
                    .get(id)
                    .and_then(Option::as_ref)
                    .and_then(|generation| RuleOriginSource::generation(generation, path))
            } else if kind == ENTITY_SENSOR_RULE_DELIVERY_CONFIG {
                self.deliveries
                    .get(id)
                    .and_then(Option::as_ref)
                    .map(RuleOriginSource::delivery)
            } else {
                None
            };
            let Some(source) = source else { continue };
            if self.rules.get(&source.rule()).and_then(Option::as_deref) != Some(source.reference())
            {
                continue;
            }
            if let Some(generation) = self
                .generations
                .get(&source.generation_id())
                .and_then(Option::as_ref)
            {
                let actual_pack = self.rule_pack_ids.get(&source.rule()).copied();
                requests.push((source, generation, actual_pack));
            }
        }
        self.sensor_rule_proofs.load_many(db, &requests).await
    }

    async fn load_events(
        &mut self,
        db: &sqlx::PgPool,
        user: &AuthenticatedUser,
        grants: &[Grant],
        ids: &[i64],
    ) -> ApiResult<()> {
        let missing = ids
            .iter()
            .copied()
            .filter(|id| !self.events.contains_key(id))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(());
        }
        let mut rows = HashMap::<i64, Vec<EventDisclosureRow>>::new();
        for row in EventRepository::find_disclosure_rows(db, &missing).await? {
            rows.entry(row.id).or_default().push(row);
        }
        for id in &missing {
            // Native event keys are (id, created). Never overwrite duplicate IDs
            // or choose an arbitrary partition's row as disclosure authority.
            let row = rows
                .remove(id)
                .filter(|rows| rows.len() == 1)
                .and_then(|mut rows| rows.pop());
            self.events.insert(*id, row);
        }
        let retained = missing
            .iter()
            .copied()
            .filter(|id| self.events.get(id).and_then(Option::as_ref).is_some())
            .collect::<Vec<_>>();
        for (id, binding) in EventSchemaRepository::find_many(db, &retained).await? {
            self.event_bindings.insert(id, binding);
        }
        let needs_read = matches!(
            crate::routes::events::event_action_visibility_filter(
                user,
                grants,
                attune_common::rbac::Action::Read
            ),
            Ok(Some(_))
        );
        let needs_decrypt = matches!(
            crate::routes::events::event_action_visibility_filter(
                user,
                grants,
                attune_common::rbac::Action::Decrypt
            ),
            Ok(Some(_))
        );
        if let Some(public) = (needs_read || needs_decrypt)
            .then_some(needs_read && user.claims.token_type == TokenType::Access)
        {
            let visible = EventRepository::visible_ids(
                db,
                &retained,
                &crate::routes::events::event_visibility_filter_from_grants(grants, public),
            )
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
            for id in &missing {
                self.event_visibility
                    .insert((*id, public), visible.contains(id));
            }
        }
        Ok(())
    }

    async fn load_generations(&mut self, db: &sqlx::PgPool, ids: &[i64]) -> ApiResult<()> {
        let mut missing = ids
            .iter()
            .copied()
            .filter(|id| !self.generations.contains_key(id))
            .collect::<Vec<_>>();
        missing.sort_unstable();
        missing.dedup();
        for batch in missing.chunks(SOURCE_BATCH_SIZE) {
            self.sensor_rule_proofs
                .prepare_generations(db, batch)
                .await?;
            let rows = SensorSchemaRepository::find_disclosure_rows(db, batch).await?;
            for id in batch {
                self.generations.insert(*id, None);
            }
            for row in rows {
                if self.sensor_rule_proofs.generation_matches(&row) {
                    self.generations.insert(row.id, Some(row));
                }
            }
        }
        Ok(())
    }

    async fn load_rules(&mut self, db: &sqlx::PgPool, ids: &[i64]) -> ApiResult<()> {
        let mut missing = ids
            .iter()
            .copied()
            .filter(|id| !self.rules.contains_key(id))
            .collect::<Vec<_>>();
        missing.sort_unstable();
        missing.dedup();
        for batch in missing.chunks(SOURCE_BATCH_SIZE) {
            let rows = RuleRepository::find_disclosure_refs(db, batch).await?;
            for id in batch {
                self.rules.insert(*id, None);
            }
            for (id, reference, managing_pack) in rows {
                if let Some(pack) = managing_pack {
                    self.rule_pack_ids.insert(id, pack);
                }
                self.rules.insert(id, Some(reference));
            }
        }
        Ok(())
    }
}

fn supported_entity(kind: &str) -> bool {
    matches!(
        kind,
        ENTITY_EXECUTION_CONFIG
            | attune_common::repositories::event_origin::EVENT_SOURCE_PAYLOAD
            | attune_common::repositories::event_origin::EVENT_SOURCE_CONFIG
            | attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ
            | ENTITY_EXECUTION_RESULT
            | "workflow_variables"
            | ENTITY_INQUIRY_OPTIONS
            | ENTITY_INQUIRY_RESPONSE
            | ENTITY_COMPONENT_CONFIGURATION
            | ENTITY_SENSOR_GENERATION_CONFIG
            | ENTITY_SENSOR_GENERATION_RULE_CONFIG
            | ENTITY_SENSOR_RULE_DELIVERY_CONFIG
    )
}
