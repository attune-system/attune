//! Structural checks for retained sensor-rule origins, independent of grants.
use super::{
    runtime_target::ResolvedRuntimeTarget, sensor_rule_origin::SensorRuleOriginRepository,
};
use crate::{models::Id, sensor_schema::SensorGenerationBinding, Error, Result};
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};

/// C's typed generic frontier only. Ordinary-human all-source loading stays G's
/// separate loader. None entries retain negative lookups for this transaction.
#[derive(Default)]
pub struct SensorOriginFrontier {
    proofs: BTreeMap<
        super::sensor_rule_origin::SensorRuleOriginKey,
        Option<super::sensor_rule_origin::SensorRuleOriginProof>,
    >,
    bindings: BTreeMap<Id, Option<super::managed_sensor_metadata::PreparedSensorGenerationBinding>>,
    initial: BTreeMap<Id, Option<serde_json::Value>>,
    deliveries: BTreeMap<Id, Option<super::sensor_schema::SensorDeliveryDisclosureRow>>,
    live_checked: BTreeSet<Id>,
    retained_bytes: usize,
}

impl SensorOriginFrontier {
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
    pub async fn prepare_provenance(
        &mut self,
        connection: &mut PgConnection,
        frontier: &[crate::secret_provenance::SecretProvenance],
    ) -> Result<()> {
        use super::sensor_schema::{
            ENTITY_SENSOR_GENERATION_CONFIG, ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        };
        let mut nodes = Vec::new();
        for provenance in frontier {
            for origin in &provenance.origins {
                if let crate::secret_provenance::SecretOrigin::Entity {
                    entity_type,
                    entity_id,
                    path,
                } = origin
                {
                    if matches!(
                        entity_type.as_str(),
                        ENTITY_SENSOR_GENERATION_CONFIG
                            | ENTITY_SENSOR_GENERATION_RULE_CONFIG
                            | ENTITY_SENSOR_RULE_DELIVERY_CONFIG
                    ) {
                        nodes.push((entity_type.clone(), *entity_id, path.clone()));
                        if nodes.len() > 256 {
                            return Err(unavailable());
                        }
                    }
                }
            }
        }
        self.prepare(connection, &nodes).await
    }
    pub async fn prepare(
        &mut self,
        connection: &mut PgConnection,
        nodes: &[(String, Id, String)],
    ) -> Result<()> {
        use super::{
            managed_sensor_metadata::ManagedSensorMetadataRepository as Metadata,
            sensor_schema::{
                SensorSchemaRepository as Schemas, ENTITY_SENSOR_GENERATION_CONFIG,
                ENTITY_SENSOR_GENERATION_RULE_CONFIG, ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
            },
        };
        if nodes.len() > 256 {
            return Err(unavailable());
        }
        let mut keys = BTreeSet::new();
        let mut generations = BTreeSet::new();
        let mut initial = BTreeSet::new();
        let mut deliveries = BTreeSet::new();
        for (entity, source, path) in nodes {
            if *source <= 0 {
                return Err(unavailable());
            }
            match entity.as_str() {
                ENTITY_SENSOR_GENERATION_CONFIG => {
                    generations.insert(*source);
                }
                ENTITY_SENSOR_GENERATION_RULE_CONFIG => {
                    let rule = rule_id_from_path(path).ok_or_else(unavailable)?;
                    generations.insert(*source);
                    if !self.initial.contains_key(source) {
                        initial.insert(*source);
                    }
                    keys.insert((entity.clone(), *source, rule));
                }
                ENTITY_SENSOR_RULE_DELIVERY_CONFIG => {
                    if !self.deliveries.contains_key(source) {
                        deliveries.insert(*source);
                    }
                }
                _ => return Err(unavailable()),
            }
        }
        if !deliveries.is_empty() {
            let ids = deliveries.iter().copied().collect::<Vec<_>>();
            let rows = Schemas::find_delivery_disclosure_rows(&mut *connection, &ids).await?;
            for id in ids {
                self.deliveries.insert(id, None);
            }
            for row in rows {
                self.retained_bytes = self
                    .retained_bytes
                    .checked_add(
                        serde_json::to_vec(&(
                            row.id,
                            row.generation_binding,
                            row.rule,
                            &row.rule_ref,
                            &row.pack_ref,
                        ))?
                        .len(),
                    )
                    .ok_or_else(unavailable)?;
                self.deliveries.insert(row.id, Some(row));
            }
        }
        for (entity, source, _) in nodes {
            if entity == ENTITY_SENSOR_RULE_DELIVERY_CONFIG {
                if let Some(Some(row)) = self.deliveries.get(source) {
                    generations.insert(row.generation_binding);
                    keys.insert((entity.clone(), *source, row.rule));
                }
            }
        }
        if !initial.is_empty() {
            let ids = initial.into_iter().collect::<Vec<_>>();
            let rows = Schemas::find_disclosure_rows(&mut *connection, &ids).await?;
            for id in ids {
                self.initial.insert(id, None);
            }
            for row in rows {
                self.retained_bytes = self
                    .retained_bytes
                    .checked_add(serde_json::to_vec(&row.initial_rules)?.len())
                    .ok_or_else(unavailable)?;
                self.initial.insert(row.id, Some(row.initial_rules));
            }
        }
        let missing = keys
            .into_iter()
            .filter(|key| !self.proofs.contains_key(key))
            .collect::<Vec<_>>();
        if self.proofs.len() + missing.len() > 256 {
            return Err(unavailable());
        }
        if !missing.is_empty() {
            let rows = SensorRuleOriginRepository::find_many(&mut *connection, &missing).await?;
            for key in missing {
                self.proofs.insert(key, None);
            }
            for (key, proof) in rows {
                generations.insert(proof.generation_binding);
                self.retained_bytes = self
                    .retained_bytes
                    .checked_add(serde_json::to_vec(&proof)?.len())
                    .ok_or_else(unavailable)?;
                self.proofs.insert(key, Some(proof));
            }
        }
        let missing = generations
            .into_iter()
            .filter(|id| !self.bindings.contains_key(id))
            .collect::<Vec<_>>();
        if self.bindings.len() + missing.len() > 256 {
            return Err(unavailable());
        }
        if !missing.is_empty() {
            let rows = Metadata::prepare_exact_bindings(&mut *connection, &missing).await?;
            for id in missing {
                self.bindings.insert(id, None);
            }
            for (id, prepared) in rows {
                self.retained_bytes = self
                    .retained_bytes
                    .checked_add(
                        serde_json::to_vec(&(&prepared.binding, &prepared.metadata))?.len(),
                    )
                    .ok_or_else(unavailable)?;
                self.bindings.insert(id, Some(prepared));
            }
        }
        if self.retained_bytes > 1024 * 1024 {
            return Err(unavailable());
        }
        Ok(())
    }

    pub fn delivery(&self, source: Id) -> Option<(Id, Id, String, String)> {
        self.deliveries.get(&source)?.as_ref().map(|row| {
            (
                row.generation_binding,
                row.rule,
                row.rule_ref.clone(),
                row.pack_ref.clone(),
            )
        })
    }

    pub fn initial_rule(&self, generation: Id, rule: Id) -> Option<(Id, String, String)> {
        let row = self
            .initial
            .get(&generation)?
            .as_ref()?
            .get(rule.to_string())?;
        Some((
            rule,
            row.get("ref")?.as_str()?.to_owned(),
            row.get("pack_ref")?.as_str()?.to_owned(),
        ))
    }

    pub async fn validated_binding(
        &mut self,
        connection: &mut PgConnection,
        generation: Id,
    ) -> Result<SensorGenerationBinding> {
        let binding = self
            .bindings
            .get(&generation)
            .and_then(Option::as_ref)
            .ok_or_else(unavailable)?
            .binding
            .clone();
        // Preparation is immutable evidence, not a live source permit. Preserve
        // the existing metadata-admission checks, once per actual generation.
        if !self.live_checked.contains(&generation) {
            validate_generation_binding(connection, generation, &binding).await?;
            self.live_checked.insert(generation);
        }
        Ok(binding)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rule_owner_matches(
        &self,
        entity: &str,
        source: Id,
        rule: Id,
        reference: &str,
        pack: &str,
        generation: Id,
        binding: &SensorGenerationBinding,
        target: &ResolvedRuntimeTarget,
    ) -> Result<bool> {
        let proof = self
            .proofs
            .get(&(entity.to_owned(), source, rule))
            .ok_or_else(unavailable)?;
        matches_proof(
            proof.as_ref(),
            rule,
            reference,
            pack,
            generation,
            binding,
            target,
        )
    }
}

fn unavailable() -> Error {
    Error::PermissionDenied("Retained sensor origin is unavailable or exceeds its bound".into())
}

pub fn rule_id_from_path(path: &str) -> Option<Id> {
    let head = path.strip_prefix('/')?.split('/').next()?;
    let id = head.parse::<Id>().ok().filter(|id| *id > 0)?;
    (head == id.to_string()).then_some(id)
}

/// An assigned configuration contract requires a repository-owned generation
/// link. A self-asserted metadata revision in serialized binding JSON is not proof.
pub async fn validate_generation_binding(
    connection: &mut PgConnection,
    id: Id,
    binding: &SensorGenerationBinding,
) -> Result<()> {
    binding.validate()?;
    if let Some(revision) = binding.metadata_revision {
        let (workload, generation, hash): (Id, i64, String) = sqlx::query_as(
            "SELECT workload,generation,binding_digest FROM sensor_generation_schema_binding WHERE id=$1 FOR SHARE",
        ).bind(id).fetch_one(&mut *connection).await?;
        use super::managed_sensor_metadata::ManagedSensorMetadataRepository as Metadata;
        if binding.binding_digest()? != hash
            || Metadata::revision_for_generation(connection, workload, generation).await?
                != Some(revision)
        {
            return Err(Error::PermissionDenied(
                "Sensor source metadata link is unavailable".into(),
            ));
        }
        Metadata::validate_generation(connection, workload, generation).await?;
    }
    Ok(())
}

/// The target must come from actual rows. This proof never grants Read/Decrypt;
/// every caller applies its own consumer or contributor authority separately.
#[allow(clippy::too_many_arguments)]
pub async fn rule_owner_matches(
    connection: &mut PgConnection,
    entity: &str,
    source: Id,
    rule: Id,
    reference: &str,
    pack: &str,
    generation: Id,
    binding: &SensorGenerationBinding,
    target: &ResolvedRuntimeTarget,
) -> Result<bool> {
    let proof = SensorRuleOriginRepository::find(connection, entity, source, rule).await?;
    matches_proof(
        proof.as_ref(),
        rule,
        reference,
        pack,
        generation,
        binding,
        target,
    )
}

#[allow(clippy::too_many_arguments)]
fn matches_proof(
    proof: Option<&super::sensor_rule_origin::SensorRuleOriginProof>,
    rule: Id,
    reference: &str,
    pack: &str,
    generation: Id,
    binding: &SensorGenerationBinding,
    target: &ResolvedRuntimeTarget,
) -> Result<bool> {
    if target.context().target_id != Some(rule)
        || target.context().target_ref.as_deref() != Some(reference)
        || target.context().pack_ref.as_deref() != Some(pack)
    {
        return Ok(false);
    }
    let Some(proof) = proof else {
        // Legacy records only retain the already-proven same-pack case.
        return Ok(binding.executable.sensor.pack.is_some()
            && target.pack_id() == binding.executable.sensor.pack);
    };
    proof.validate_binding(generation, binding)?;
    if proof.rule_ref != reference
        || proof.pack_ref != pack
        || target.pack_id() != Some(proof.managing_pack_id)
        || target.context().managing_pack_id != Some(proof.managing_pack_id)
    {
        return Ok(false);
    }
    if proof.managing_pack_id != proof.sensor_managing_pack_id {
        return Ok(proof.dispatch_policy_revision.is_some_and(|id| id > 0)
            && proof.dispatch_source_release.is_some_and(|id| id > 0)
            && proof.dispatch_metadata_digest.as_ref().is_some_and(|hash| {
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            }));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rule_paths_select_only_an_exact_canonical_entry_id() {
        assert_eq!(rule_id_from_path("/17/private"), Some(17));
        assert_eq!(rule_id_from_path("/17"), Some(17));
        for path in [
            "17/private",
            "x/17/private",
            "/017/private",
            "/+17/private",
            "/0/private",
            "/-1/private",
            "/~117/private",
        ] {
            assert_eq!(rule_id_from_path(path), None, "{path}");
        }
    }
}
