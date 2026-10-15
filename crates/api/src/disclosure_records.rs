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

#[derive(Default)]
pub(crate) struct DisclosureRecords {
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
}

impl DisclosureRecords {
    pub(crate) fn seed_execution_provenance(
        &mut self,
        ids: &[i64],
        rows: impl IntoIterator<Item = SecretProvenanceRow>,
    ) {
        for id in ids {
            for kind in [ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT] {
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
            if sources.is_empty() {
                break;
            }
            let mut by_kind = BTreeMap::<String, Vec<i64>>::new();
            let mut execution_ids = Vec::new();
            for (kind, id, _) in &sources {
                if !self.provenance.contains_key(&(kind.clone(), *id)) {
                    by_kind.entry(kind.clone()).or_default().push(*id);
                }
                if matches!(
                    kind.as_str(),
                    ENTITY_EXECUTION_CONFIG | ENTITY_EXECUTION_RESULT
                ) {
                    execution_ids.push(*id);
                }
            }
            self.load_execution_provenance(db, &execution_ids).await?;
            for (kind, mut ids) in by_kind {
                if matches!(
                    kind.as_str(),
                    ENTITY_EXECUTION_CONFIG | ENTITY_EXECUTION_RESULT
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
                .filter_map(|(kind, id, path)| self.parent(&kind, id, &path))
                .map(|row| (row.source_kind, row.source_ref))
                .collect();
        }
        Ok(())
    }

    async fn load_execution_provenance(&mut self, db: &sqlx::PgPool, ids: &[i64]) -> ApiResult<()> {
        let mut missing = ids
            .iter()
            .copied()
            .filter(|id| {
                [ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT]
                    .iter()
                    .any(|kind| !self.provenance.contains_key(&(kind.to_string(), *id)))
            })
            .collect::<Vec<_>>();
        missing.sort_unstable();
        missing.dedup();
        for batch in missing.chunks(SOURCE_BATCH_SIZE) {
            let rows = ExecutionSecretValueRepository::find_provenance_by_entities(
                db,
                &[ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT],
                batch,
            )
            .await?;
            self.seed_execution_provenance(batch, rows);
        }
        Ok(())
    }

    pub(crate) fn parent(&self, kind: &str, id: i64, path: &str) -> Option<SecretProvenanceRow> {
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
                            let generations = rows
                                .iter()
                                .map(|row| row.generation_binding)
                                .collect::<Vec<_>>();
                            self.load_generations(db, &generations).await?;
                            self.load_rules(
                                db,
                                &rows.iter().map(|row| row.rule).collect::<Vec<_>>(),
                            )
                            .await?;
                            for id in missing {
                                self.deliveries.insert(id, None);
                            }
                            for row in rows {
                                self.deliveries.insert(row.id, Some(row));
                            }
                        }
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
            if let Some(rule) = path
                .split('/')
                .nth(1)
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|id| *id > 0)
            {
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
            let rows = SensorSchemaRepository::find_disclosure_rows(db, batch).await?;
            for id in batch {
                self.generations.insert(*id, None);
            }
            for row in rows {
                self.generations.insert(row.id, Some(row));
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
            for (id, reference) in rows {
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
            | ENTITY_EXECUTION_RESULT
            | ENTITY_EVENT_PAYLOAD
            | ENTITY_INQUIRY_OPTIONS
            | ENTITY_INQUIRY_RESPONSE
            | ENTITY_COMPONENT_CONFIGURATION
            | ENTITY_SENSOR_GENERATION_CONFIG
            | ENTITY_SENSOR_GENERATION_RULE_CONFIG
            | ENTITY_SENSOR_RULE_DELIVERY_CONFIG
    )
}
