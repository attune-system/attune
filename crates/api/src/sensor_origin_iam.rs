//! Structural evidence for the older ordinary IAM sensor-origin reader.
//! This is not C's generic runtime-read traversal or a permission cache.

use std::collections::{BTreeSet, HashMap};

use attune_common::{
    models::Id,
    repositories::{
        managed_sensor_metadata::{
            ManagedSensorMetadataRepository, PreparedSensorGenerationBinding,
        },
        sensor_rule_origin::{SensorRuleOriginProof, SensorRuleOriginRepository},
        sensor_schema::{
            SensorDisclosureRow, ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        },
    },
    sensor_schema::SensorGenerationBinding,
};

use crate::middleware::ApiResult;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuleOriginSource {
    entity: &'static str,
    id: Id,
    rule: Id,
    generation: Id,
    reference: String,
    pack_ref: String,
}

/// Only canonical positive decimal IDs select entries in the retained map.
/// Neither a ref prefix nor a current rule lookup can select a historical rule.
pub(crate) fn generation_rule_id(path: &str) -> Option<Id> {
    let value = path.strip_prefix('/')?.split('/').next()?;
    let id = value.parse::<Id>().ok().filter(|id| *id > 0)?;
    (id.to_string() == value).then_some(id)
}

impl RuleOriginSource {
    pub(crate) fn generation(row: &SensorDisclosureRow, path: &str) -> Option<Self> {
        let rule = generation_rule_id(path)?;
        let entry = row.initial_rules.get(rule.to_string())?;
        Some(Self {
            entity: ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            id: row.id,
            rule,
            generation: row.id,
            reference: entry.get("ref")?.as_str()?.into(),
            pack_ref: entry.get("pack_ref")?.as_str()?.into(),
        })
    }

    pub(crate) fn delivery(
        row: &attune_common::repositories::sensor_schema::SensorDeliveryDisclosureRow,
    ) -> Self {
        Self {
            entity: ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
            id: row.id,
            rule: row.rule,
            generation: row.generation_binding,
            reference: row.rule_ref.clone(),
            pack_ref: row.pack_ref.clone(),
        }
    }

    pub(crate) fn rule(&self) -> Id {
        self.rule
    }
    pub(crate) fn reference(&self) -> &str {
        &self.reference
    }
    pub(crate) fn generation_id(&self) -> Id {
        self.generation
    }
    pub(crate) fn pack_ref(&self) -> &str {
        &self.pack_ref
    }

    fn key(&self) -> (&'static str, Id, Id) {
        (self.entity, self.id, self.rule)
    }
}

struct RuleProofRecord {
    source: RuleOriginSource,
    actual_pack: Option<Id>,
    proven_pack: Option<Id>,
}

#[derive(Default)]
pub(crate) struct SensorRuleProofCache {
    records: HashMap<(&'static str, Id, Id), RuleProofRecord>,
    generations: HashMap<Id, Option<PreparedSensorGenerationBinding>>,
}

impl SensorRuleProofCache {
    #[cfg(test)]
    pub(crate) async fn prepare_generations(
        &mut self,
        db: &sqlx::PgPool,
        ids: &[Id],
    ) -> ApiResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut connection = db.acquire().await?;
        self.prepare_generations_on(&mut connection, ids).await
    }

    pub(crate) async fn prepare_generations_on(
        &mut self,
        db: &mut sqlx::PgConnection,
        ids: &[Id],
    ) -> ApiResult<()> {
        let missing = ids
            .iter()
            .copied()
            .filter(|id| !self.generations.contains_key(id))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        for batch in missing.chunks(64) {
            let rows =
                ManagedSensorMetadataRepository::prepare_exact_bindings(&mut *db, batch).await?;
            for id in batch {
                self.generations.insert(*id, None);
            }
            for (id, prepared) in rows {
                self.generations.insert(id, Some(prepared));
            }
        }
        Ok(())
    }

    pub(crate) fn generation_matches(&self, generation: &SensorDisclosureRow) -> bool {
        self.generations
            .get(&generation.id)
            .and_then(Option::as_ref)
            .is_some_and(|prepared| same_binding(&prepared.binding, &generation.binding))
    }
    #[cfg(test)]
    pub(crate) async fn load_many(
        &mut self,
        db: &sqlx::PgPool,
        requests: &[(RuleOriginSource, &SensorDisclosureRow, Option<Id>)],
    ) -> ApiResult<()> {
        if requests.is_empty() {
            return Ok(());
        }
        let mut connection = db.acquire().await?;
        self.load_many_on(&mut connection, requests).await
    }

    pub(crate) async fn load_many_on(
        &mut self,
        db: &mut sqlx::PgConnection,
        requests: &[(RuleOriginSource, &SensorDisclosureRow, Option<Id>)],
    ) -> ApiResult<()> {
        let mut keys = BTreeSet::new();
        for (source, generation, actual_pack) in requests {
            if !self.records.contains_key(&source.key())
                && !same_pack_source(source, generation, *actual_pack)
                && actual_pack.is_some()
            {
                keys.insert((source.entity.to_string(), source.id, source.rule));
            }
        }
        let mut proofs = std::collections::BTreeMap::new();
        for batch in keys.into_iter().collect::<Vec<_>>().chunks(64) {
            proofs.extend(SensorRuleOriginRepository::find_many(&mut *db, batch).await?);
        }
        for (source, generation, actual_pack) in requests {
            if self.records.contains_key(&source.key()) {
                continue;
            }
            self.cache_source(
                source,
                generation,
                *actual_pack,
                proofs.get(&(source.entity.to_string(), source.id, source.rule)),
            );
        }
        Ok(())
    }

    fn cache_source(
        &mut self,
        source: &RuleOriginSource,
        generation: &SensorDisclosureRow,
        actual_pack: Option<Id>,
        proof: Option<&SensorRuleOriginProof>,
    ) {
        let mut proven_pack = None;
        if source.generation == generation.id {
            if let Some(binding) = self
                .generations
                .get(&generation.id)
                .and_then(Option::as_ref)
                .map(|prepared| &prepared.binding)
                .filter(|binding| same_binding(binding, &generation.binding))
            {
                if same_pack_source(source, generation, actual_pack) {
                    proven_pack = actual_pack;
                } else if actual_pack.is_some()
                    && generation.managing_pack_id == binding.executable.sensor.pack
                {
                    if let Some(proof) = proof {
                        if matches_retained_source(proof, source, actual_pack)
                            && proof.validate_binding(source.generation, binding).is_ok()
                            && Some(proof.sensor_managing_pack_id) == generation.managing_pack_id
                        {
                            proven_pack = Some(proof.managing_pack_id);
                        }
                    }
                }
            }
        }
        self.records.insert(
            source.key(),
            RuleProofRecord {
                source: source.clone(),
                actual_pack,
                proven_pack,
            },
        );
    }

    pub(crate) fn pack_for(
        &self,
        source: &RuleOriginSource,
        actual_pack: Option<Id>,
    ) -> Option<Id> {
        self.records
            .get(&source.key())
            .filter(|record| record.source == *source && record.actual_pack == actual_pack)?
            .proven_pack
    }
}

fn same_pack_source(
    source: &RuleOriginSource,
    generation: &SensorDisclosureRow,
    actual_pack: Option<Id>,
) -> bool {
    actual_pack.is_some()
        && actual_pack == generation.managing_pack_id
        && generation.binding.executable.sensor.pack == actual_pack
        && generation.binding.executable.sensor.pack_ref.as_deref()
            == Some(source.pack_ref.as_str())
}

fn same_binding(left: &SensorGenerationBinding, right: &SensorGenerationBinding) -> bool {
    left.binding_digest()
        .ok()
        .zip(right.binding_digest().ok())
        .is_some_and(|(left, right)| left == right)
}

fn matches_retained_source(
    proof: &SensorRuleOriginProof,
    source: &RuleOriginSource,
    actual_pack: Option<Id>,
) -> bool {
    proof.source_entity == source.entity
        && proof.source_id == source.id
        && proof.rule_id == source.rule
        && proof.generation_binding == source.generation
        && proof.rule_ref == source.reference
        && proof.pack_ref == source.pack_ref
        && Some(proof.managing_pack_id) == actual_pack
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::rbac::{Action, AuthorizationContext, Grant, Resource};

    #[tokio::test]
    async fn empty_historical_frontier_acquires_no_database_connection() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(1))
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let mut cache = SensorRuleProofCache::default();
        cache.prepare_generations(&pool, &[]).await.unwrap();
        cache.load_many(&pool, &[]).await.unwrap();
        assert!(cache.generations.is_empty());
        assert!(cache.records.is_empty());
        pool.close().await;
    }

    fn proof() -> SensorRuleOriginProof {
        SensorRuleOriginProof {
            source_entity: ENTITY_SENSOR_RULE_DELIVERY_CONFIG.into(),
            source_id: 10,
            rule_id: 20,
            generation_binding: 30,
            rule_ref: "foreign.rule".into(),
            managing_pack_id: 40,
            pack_ref: "foreign".into(),
            trigger_id: 50,
            trigger_ref: "local.trigger".into(),
            trigger_managing_pack_id: Some(60),
            sensor_id: 70,
            sensor_managing_pack_id: 60,
            dispatch_policy_revision: Some(80),
            dispatch_source_release: Some(90),
            dispatch_metadata_digest: Some("a".repeat(64)),
            parameter_contract_digest: None,
        }
    }
    fn source() -> RuleOriginSource {
        RuleOriginSource {
            entity: ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
            id: 10,
            rule: 20,
            generation: 30,
            reference: "foreign.rule".into(),
            pack_ref: "foreign".into(),
        }
    }

    #[test]
    fn foreign_proof_matches_only_exact_retained_source_and_actual_owner_and_is_not_permission() {
        let proof = proof();
        let source = source();
        assert!(matches_retained_source(&proof, &source, Some(40)));
        assert!(!matches_retained_source(&proof, &source, None));
        assert!(!matches_retained_source(&proof, &source, Some(41)));
        let mut context = AuthorizationContext::new(1);
        context.managing_pack_id = Some(40);
        context.target_id = Some(20);
        context.target_ref = Some(source.reference.clone());
        let grants:Vec<Grant>=serde_json::from_value(serde_json::json!([{"resource":"rules","actions":["read"],"constraints":{"managing_pack_id":60}}])).unwrap();
        assert!(!grants
            .iter()
            .any(|grant| grant.allows(Resource::Rules, Action::Read, &context)));
        let local_rights: Vec<Grant> = serde_json::from_value(serde_json::json!([
            {"resource":"rules","actions":["read","decrypt"],"constraints":{"managing_pack_id":40,"ids":[20],"refs":["foreign.rule"]}}
        ])).unwrap();
        for action in [Action::Read, Action::Decrypt] {
            assert!(local_rights.iter().any(|grant| grant.allows(
                Resource::Rules,
                action,
                &context
            )));
        }
        context.target_id = Some(21);
        assert!(!local_rights.iter().any(|grant| grant.allows(
            Resource::Rules,
            Action::Read,
            &context
        )));
        let mut cache = SensorRuleProofCache::default();
        cache.records.insert(
            source.key(),
            RuleProofRecord {
                source: source.clone(),
                actual_pack: Some(40),
                proven_pack: Some(40),
            },
        );
        assert_eq!(cache.pack_for(&source, Some(40)), Some(40));
        assert_eq!(cache.pack_for(&source, Some(41)), None);
        let mut changed_proof = proof.clone();
        changed_proof.managing_pack_id = 41;
        assert!(!matches_retained_source(&changed_proof, &source, Some(40)));
        for mutate in [0, 1, 2, 3, 4] {
            let mut changed = source.clone();
            match mutate {
                0 => changed.id += 1,
                1 => changed.rule += 1,
                2 => changed.generation += 1,
                3 => changed.reference = "reused.rule".into(),
                _ => changed.pack_ref = "reused".into(),
            };
            assert!(!matches_retained_source(&proof, &changed, Some(40)));
        }
        assert_eq!(
            SensorRuleProofCache::default().pack_for(&source, Some(40)),
            None
        );
    }

    #[test]
    fn generation_rule_paths_cannot_select_ids_through_prefixes_or_ambiguous_decimal_forms() {
        assert_eq!(generation_rule_id("/20/settings/token"), Some(20));
        for path in [
            "20/settings",
            "/020/settings",
            "/+20/settings",
            "/-20/settings",
            "/0/settings",
            "/20~1settings/token",
            "/18446744073709551616/settings",
        ] {
            assert_eq!(generation_rule_id(path), None, "{path}");
        }
    }
}
