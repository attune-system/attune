//! Creation-time structural proof for foreign rule configuration disclosure.
use crate::{
    models::{Id, Rule},
    value_schema::{digest, Contract},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, PgConnection, Postgres};
use std::collections::{BTreeMap, BTreeSet};

/// Exact source identity, never three independently matched selector sets.
pub type SensorRuleOriginKey = (String, Id, Id);
pub type SensorRuleOriginMap = BTreeMap<SensorRuleOriginKey, SensorRuleOriginProof>;
/// Shared proof/binding input cap, before duplicate coalescing. This does not
/// raise a caller's traversal/node/byte limits or its smaller frontier size.
pub const MAX_SENSOR_ORIGIN_BATCH: usize = 256;

#[derive(sqlx::FromRow)]
struct ProofRow {
    source_entity: String,
    source_id: Id,
    rule_id: Id,
    proof: serde_json::Value,
    proof_digest: String,
    dispatch_policy_revision: Option<Id>,
    generation_binding: Id,
    dispatch_source_release: Option<Id>,
}

fn valid_key(key: &SensorRuleOriginKey) -> bool {
    matches!(
        key.0.as_str(),
        "sensor_generation_rule_config" | "sensor_rule_delivery_config"
    ) && key.1 > 0
        && key.2 > 0
}

fn valid_digest(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode(row: ProofRow) -> Result<(SensorRuleOriginKey, SensorRuleOriginProof)> {
    let key = (row.source_entity, row.source_id, row.rule_id);
    if !valid_key(&key) || digest(&row.proof).ok().as_ref() != Some(&row.proof_digest) {
        return Err(Error::validation("Invalid sensor rule origin proof"));
    }
    let proof: SensorRuleOriginProof = serde_json::from_value(row.proof)
        .map_err(|_| Error::validation("Invalid sensor rule origin proof"))?;
    proof.validate_shape()?;
    if proof.source_entity != key.0
        || proof.source_id != key.1
        || proof.rule_id != key.2
        || proof.dispatch_policy_revision != row.dispatch_policy_revision
        || proof.dispatch_source_release != row.dispatch_source_release
        || proof.generation_binding != row.generation_binding
    {
        return Err(Error::validation(
            "Sensor rule origin proof ownership mismatch",
        ));
    }
    Ok((key, proof))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorRuleOriginProof {
    pub source_entity: String,
    pub source_id: Id,
    pub rule_id: Id,
    pub generation_binding: Id,
    pub rule_ref: String,
    pub managing_pack_id: Id,
    pub pack_ref: String,
    pub trigger_id: Id,
    pub trigger_ref: String,
    pub trigger_managing_pack_id: Option<Id>,
    pub sensor_id: Id,
    pub sensor_managing_pack_id: Id,
    pub dispatch_policy_revision: Option<Id>,
    pub dispatch_source_release: Option<Id>,
    pub dispatch_metadata_digest: Option<String>,
    pub parameter_contract_digest: Option<String>,
}

pub struct SensorRuleOriginRepository;

impl SensorRuleOriginProof {
    pub fn validate_shape(&self) -> Result<()> {
        if !valid_key(&(self.source_entity.clone(), self.source_id, self.rule_id))
            || self.generation_binding <= 0
            || self.managing_pack_id <= 0
            || self.trigger_managing_pack_id.is_some_and(|id| id <= 0)
            || self.sensor_managing_pack_id <= 0
            || self.sensor_id <= 0
            || self.trigger_id <= 0
            || self.rule_ref.is_empty()
            || self.pack_ref.is_empty()
            || self.trigger_ref.is_empty()
            || (self.source_entity == "sensor_generation_rule_config"
                && self.source_id != self.generation_binding)
            || self.dispatch_policy_revision.is_some_and(|id| id <= 0)
            || self.dispatch_source_release.is_some_and(|id| id <= 0)
            || !matches!(
                (
                    self.dispatch_policy_revision,
                    self.dispatch_source_release,
                    self.dispatch_metadata_digest.as_ref()
                ),
                (None, None, None) | (Some(_), Some(_), Some(_))
            )
            || self
                .dispatch_metadata_digest
                .as_ref()
                .is_some_and(|hash| !valid_digest(hash))
            || self
                .parameter_contract_digest
                .as_ref()
                .is_some_and(|hash| !valid_digest(hash))
            || (self.managing_pack_id != self.sensor_managing_pack_id
                && self.dispatch_policy_revision.is_none())
        {
            return Err(Error::validation("Invalid sensor rule origin proof shape"));
        }
        Ok(())
    }
    /// Match only retained identities and contracts. No live relationship lookup
    /// can supply missing evidence for a historical source.
    pub fn validate_binding(
        &self,
        generation_id: Id,
        binding: &crate::sensor_schema::SensorGenerationBinding,
    ) -> Result<()> {
        self.validate_shape()?;
        binding.validate()?;
        let trigger = binding
            .executable
            .triggers
            .iter()
            .find(|trigger| trigger.id == self.trigger_id && trigger.r#ref == self.trigger_ref)
            .ok_or_else(|| {
                Error::validation("Rule origin trigger is absent from retained generation")
            })?;
        let admitted = binding
            .triggers
            .get(&self.trigger_ref)
            .filter(|trigger| trigger.event.trigger_id == Some(self.trigger_id))
            .ok_or_else(|| {
                Error::validation("Rule origin trigger schema is absent from retained generation")
            })?;
        if self.generation_binding != generation_id
            || self.sensor_id != binding.executable.sensor.id
            || Some(self.sensor_managing_pack_id) != binding.executable.sensor.pack
            || self.parameter_contract_digest.as_ref()
                != admitted
                    .parameters
                    .as_ref()
                    .map(|contract| &contract.binding_digest)
            || self
                .trigger_managing_pack_id
                .is_some_and(|pack| trigger.pack != Some(pack))
            || (self.managing_pack_id != self.sensor_managing_pack_id
                && self.dispatch_policy_revision.is_none())
        {
            return Err(Error::validation(
                "Rule origin does not match retained sensor generation",
            ));
        }
        Ok(())
    }
}

impl SensorRuleOriginRepository {
    pub(super) async fn capture(
        connection: &mut PgConnection,
        entity: &str,
        id: Id,
        rule: &Rule,
        contract: Option<&Contract>,
        sensor_id: Id,
        sensor_pack: Id,
    ) -> Result<()> {
        // Only the producer's own new immutable source may acquire proof. Reading
        // today's same-name pack/rule cannot upgrade retained historical records.
        let query = match entity {
            "sensor_generation_rule_config" => "SELECT EXISTS(SELECT 1 FROM sensor_generation_schema_binding WHERE id=$1 AND xmin::text::numeric=mod(pg_current_xact_id()::text::numeric,4294967296))",
            "sensor_rule_delivery_config" => "SELECT EXISTS(SELECT 1 FROM sensor_rule_config_delivery WHERE id=$1 AND xmin::text::numeric=mod(pg_current_xact_id()::text::numeric,4294967296))",
            _ => return Err(Error::validation("Unsupported sensor rule origin source")),
        };
        let fresh: bool = sqlx::query_scalar(query)
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
        if !fresh {
            return Err(Error::PermissionDenied(
                "Historical sensor rule sources cannot acquire new structural proof".into(),
            ));
        }
        use super::runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository};
        let sponsor = super::runtime_authority::RuntimeAuthorityRepository::ensure_owner(
            connection,
            sensor_pack,
        )
        .await?;
        let rule_target = RuntimeTargetRepository::resolve(
            connection,
            RuntimeTarget::Component(ComponentTarget::Rule, rule.id),
            sponsor.sponsor_identity_id,
        )
        .await?
        .ok_or_else(|| Error::validation("Rule structural source is unavailable"))?;
        // Pack association is not structural ownership. Unmanaged sources retain
        // no invented local proof; historical disclosure continues to fail closed.
        let Some(rule_pack) = rule_target.pack_id() else {
            return Ok(());
        };
        if rule_pack != rule.pack {
            return Err(Error::validation("Rule structural owner mismatch"));
        }
        let sensor_target = RuntimeTargetRepository::resolve(
            connection,
            RuntimeTarget::Component(ComponentTarget::Sensor, sensor_id),
            sponsor.sponsor_identity_id,
        )
        .await?
        .ok_or_else(|| Error::validation("Sensor structural source is unavailable"))?;
        if sensor_target.pack_id() != Some(sensor_pack) {
            return Err(Error::validation("Sensor structural owner mismatch"));
        }
        let trigger_id = rule
            .trigger
            .ok_or_else(|| Error::validation("Rule trigger is unavailable"))?;
        let trigger_target = RuntimeTargetRepository::resolve(
            connection,
            RuntimeTarget::Component(ComponentTarget::Trigger, trigger_id),
            sponsor.sponsor_identity_id,
        )
        .await?
        .ok_or_else(|| Error::validation("Trigger structural source is unavailable"))?;
        let trigger_pack = trigger_target.pack_id();
        let owned: Option<Id> =
            sqlx::query_scalar("SELECT id FROM pack WHERE id=$1 AND ref=$2 FOR SHARE")
                .bind(rule.pack)
                .bind(&rule.pack_ref)
                .fetch_optional(&mut *connection)
                .await?;
        let current = super::rule::RuleRepository::find_by_id_for_share(connection, rule.id)
            .await?
            .ok_or_else(|| Error::validation("Sensor rule structural source is unavailable"))?;
        if owned != Some(rule.pack)
            || current.r#ref != rule.r#ref
            || current.pack != rule.pack
            || current.pack_ref != rule.pack_ref
            || current.trigger != rule.trigger
            || current.trigger_ref != rule.trigger_ref
        {
            return Err(Error::validation(
                "Sensor rule structural owner is unavailable",
            ));
        }
        let managed: bool =
            sqlx::query_scalar("SELECT management_origin='pack' FROM rule WHERE id=$1")
                .bind(rule.id)
                .fetch_one(&mut *connection)
                .await?;
        let dispatch = if managed && rule.pack != sensor_pack {
            let available: bool = sqlx::query_scalar(
                "SELECT to_regclass('managed_dispatch_metadata_revision') IS NOT NULL",
            )
            .fetch_one(&mut *connection)
            .await?;
            if !available {
                return Err(Error::PermissionDenied("Foreign managed rule requires retained dispatch metadata before sensor delivery".into()));
            }
            let row:Option<(Id,Id,serde_json::Value,String)>=sqlx::query_as("SELECT d.policy_revision,d.source_release,d.metadata,d.metadata_digest FROM managed_dispatch_metadata_revision d JOIN pack_runtime_policy_current c ON c.revision=d.policy_revision JOIN pack_runtime_policy_revision p ON p.id=c.revision WHERE d.component_kind='rules' AND d.component_id=$1 AND c.pack=$2 AND p.pack=$2 AND p.revoked_at IS NULL AND EXISTS(SELECT 1 FROM jsonb_array_elements(p.policy->'invocations') entry WHERE (entry->>'source_id')::BIGINT=d.component_id AND entry->'binding'->>'kind'='rule' AND entry->'binding'->>'rule_ref'=d.metadata->>'rule_ref' AND entry->>'dispatch_template_digest'=d.metadata_digest) FOR SHARE OF d,c,p")
                .bind(rule.id).bind(rule.pack).fetch_optional(&mut *connection).await?;
            let (policy, release, body, hash) = row.ok_or_else(|| {
                Error::PermissionDenied(
                    "Foreign sensor rule has no retained dispatch source proof".into(),
                )
            })?;
            if digest(&body).map_err(|_| Error::validation("Invalid retained rule source"))? != hash
                || body["rule_id"].as_i64() != Some(rule.id)
                || body["rule_ref"].as_str() != Some(rule.r#ref.as_str())
                || body["pack_ref"].as_str() != Some(rule.pack_ref.as_str())
                || body["managing_pack_id"].as_i64() != Some(rule.pack)
                || body["trigger"].as_i64() != rule.trigger
                || body["trigger_ref"].as_str() != Some(rule.trigger_ref.as_str())
                || body.get("trigger_managing_pack_id").is_none()
                || body["trigger_managing_pack_id"].as_i64() != trigger_pack
                || body["sensor_id"].as_i64() != Some(sensor_id)
                || body["sensor_managing_pack_id"].as_i64() != Some(sensor_pack)
                || body["trigger_params"] != rule.trigger_params
                || body["action"].as_i64() != rule.action
                || body["action_ref"].as_str() != Some(rule.action_ref.as_str())
            {
                return Err(Error::PermissionDenied(
                    "Foreign sensor rule differs from its retained dispatch source".into(),
                ));
            }
            Some((policy, release, hash))
        } else {
            None
        };
        let generation_binding = if entity == "sensor_generation_rule_config" {
            id
        } else {
            sqlx::query_scalar::<_, Id>(
                "SELECT generation_binding FROM sensor_rule_config_delivery WHERE id=$1",
            )
            .bind(id)
            .fetch_one(&mut *connection)
            .await?
        };
        let proof = SensorRuleOriginProof {
            source_entity: entity.into(),
            source_id: id,
            rule_id: rule.id,
            generation_binding,
            rule_ref: rule.r#ref.clone(),
            managing_pack_id: rule.pack,
            pack_ref: rule.pack_ref.clone(),
            trigger_id: rule
                .trigger
                .ok_or_else(|| Error::validation("Rule trigger is unavailable"))?,
            trigger_ref: rule.trigger_ref.clone(),
            trigger_managing_pack_id: trigger_pack,
            sensor_id,
            sensor_managing_pack_id: sensor_pack,
            dispatch_policy_revision: dispatch.as_ref().map(|entry| entry.0),
            dispatch_source_release: dispatch.as_ref().map(|entry| entry.1),
            dispatch_metadata_digest: dispatch.map(|entry| entry.2),
            parameter_contract_digest: contract.map(|contract| contract.binding_digest.clone()),
        };
        let body = serde_json::to_value(&proof)?;
        let (binding, _) = super::sensor_schema::SensorSchemaRepository::find_by_binding_id(
            &mut *connection,
            generation_binding,
        )
        .await?
        .ok_or_else(|| Error::validation("Rule origin generation is unavailable"))?;
        proof.validate_binding(generation_binding, &binding)?;
        let hash =
            digest(&body).map_err(|_| Error::validation("Invalid sensor rule origin proof"))?;
        sqlx::query("INSERT INTO sensor_rule_structural_origin(source_entity,source_id,rule_id,generation_binding,dispatch_policy_revision,dispatch_source_release,proof,proof_digest) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(entity).bind(id).bind(rule.id).bind(generation_binding).bind(proof.dispatch_policy_revision).bind(proof.dispatch_source_release).bind(body).bind(hash).execute(connection).await?;
        Ok(())
    }

    pub async fn find<'e, E>(
        executor: E,
        entity: &str,
        id: Id,
        rule: Id,
    ) -> Result<Option<SensorRuleOriginProof>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let key = (entity.to_owned(), id, rule);
        Ok(Self::find_many(executor, std::slice::from_ref(&key))
            .await?
            .remove(&key))
    }

    /// One statement for up to MAX_SENSOR_ORIGIN_BATCH exact tuples, including
    /// duplicates. Empty input executes no SQL. Missing tuples remain absent.
    pub async fn find_many<'e, E>(
        executor: E,
        keys: &[SensorRuleOriginKey],
    ) -> Result<SensorRuleOriginMap>
    where
        E: Executor<'e, Database = Postgres>,
    {
        if keys.len() > MAX_SENSOR_ORIGIN_BATCH || keys.iter().any(|key| !valid_key(key)) {
            return Err(Error::validation("Invalid sensor rule origin batch"));
        }
        let keys: BTreeSet<_> = keys.iter().cloned().collect();
        if keys.is_empty() {
            return Ok(BTreeMap::new());
        }
        let entities: Vec<_> = keys.iter().map(|key| key.0.clone()).collect();
        let ids: Vec<_> = keys.iter().map(|key| key.1).collect();
        let rules: Vec<_> = keys.iter().map(|key| key.2).collect();
        let rows: Vec<ProofRow> = sqlx::query_as("SELECT p.source_entity,p.source_id,p.rule_id,p.proof,p.proof_digest,p.dispatch_policy_revision,p.generation_binding,p.dispatch_source_release FROM unnest($1::TEXT[],$2::BIGINT[],$3::BIGINT[]) AS requested(entity,id,rule) JOIN sensor_rule_structural_origin p ON (p.source_entity,p.source_id,p.rule_id)=(requested.entity,requested.id,requested.rule)")
            .bind(entities).bind(ids).bind(rules).fetch_all(executor).await?;
        rows.into_iter().map(decode).collect()
    }
}
