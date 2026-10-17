//! Exact partition identity, retained schema and immutable encrypted source fields.
use crate::{
    event_schema::EventSchemaBinding,
    models::{Event, Id},
    secret_provenance::SecretProvenance,
    secret_values::{PreparedSecretValue, StoredSecretValue},
    value_schema::digest,
    Error, Result,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, PgConnection, Postgres};

pub const EVENT_SOURCE_PAYLOAD: &str = "event_source_payload";
pub const EVENT_SOURCE_CONFIG: &str = "event_source_config";
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventOriginSnapshot {
    pub event: Id,
    pub created: DateTime<Utc>,
    pub trigger: Option<Id>,
    pub trigger_ref: String,
    pub managing_pack: Option<Id>,
    pub binding: EventSchemaBinding,
    pub payload: Vec<PreparedSecretValue>,
    pub config: Vec<PreparedSecretValue>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventOriginEvidence {
    pub event: Id,
    pub created: DateTime<Utc>,
    pub trigger: Option<Id>,
    pub trigger_ref: String,
    pub managing_pack: Option<Id>,
    pub binding: EventSchemaBinding,
    pub payload: std::collections::BTreeMap<String, SecretProvenance>,
    pub config: std::collections::BTreeMap<String, SecretProvenance>,
}

impl EventOriginEvidence {
    pub fn validate(&self) -> Result<()> {
        self.binding.validate()?;
        if self.event <= 0
            || !self
                .binding
                .matches_identity(self.trigger, &self.trigger_ref)
            || self.payload.len() + self.config.len() > 256
            || serde_json::to_vec(self)?.len() > 1024 * 1024
            || self
                .payload
                .values()
                .chain(self.config.values())
                .any(|parent| parent.origins.is_empty() || parent.origins.len() > 256)
        {
            return Err(denied());
        }
        Ok(())
    }
    pub fn matches(&self, event: &Event) -> bool {
        self.event == event.id
            && self.created == event.created
            && self.trigger == event.trigger
            && self.trigger_ref == event.trigger_ref
            && self.binding.matches_owner(event)
    }
    pub fn parent(&self, section: &str, path: &str) -> Result<&SecretProvenance> {
        match section {
            EVENT_SOURCE_PAYLOAD => self.payload.get(path),
            EVENT_SOURCE_CONFIG => self.config.get(path),
            _ => None,
        }
        .ok_or_else(denied)
    }
}

impl EventOriginSnapshot {
    pub fn evidence(&self) -> Result<EventOriginEvidence> {
        let mut sections = [
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::new(),
        ];
        for (result, values) in sections.iter_mut().zip([&self.payload, &self.config]) {
            for value in values {
                if value.source_kind != "provenance"
                    || result
                        .insert(
                            value.json_path.clone(),
                            serde_json::from_str::<SecretProvenance>(
                                value.source_ref.as_deref().ok_or_else(denied)?,
                            )
                            .map_err(|_| denied())?,
                        )
                        .is_some()
                {
                    return Err(denied());
                }
            }
        }
        let [payload, config] = sections;
        let evidence = EventOriginEvidence {
            event: self.event,
            created: self.created,
            trigger: self.trigger,
            trigger_ref: self.trigger_ref.clone(),
            managing_pack: self.managing_pack,
            binding: self.binding.clone(),
            payload,
            config,
        };
        evidence.validate()?;
        Ok(evidence)
    }
    pub fn matches(&self, event: &Event) -> bool {
        self.event == event.id
            && self.created == event.created
            && self.trigger == event.trigger
            && self.trigger_ref == event.trigger_ref
            && self.binding.matches_owner(event)
    }

    pub fn validate(&self) -> Result<()> {
        self.binding.validate()?;
        self.evidence()?;
        if self.event <= 0
            || !self
                .binding
                .matches_identity(self.trigger, &self.trigger_ref)
            || self.payload.len() + self.config.len() > 256
            || serde_json::to_vec(self)?.len() > MAX_SNAPSHOT_BYTES
        {
            return Err(denied());
        }
        for values in [&self.payload, &self.config] {
            let mut paths = std::collections::BTreeSet::new();
            for value in values {
                if !paths.insert(&value.json_path)
                    || value.source_kind != "provenance"
                    || value
                        .source_ref
                        .as_ref()
                        .is_none_or(|reference| reference.len() > 1024 * 1024)
                {
                    return Err(denied());
                }
                let parent: SecretProvenance =
                    serde_json::from_str(value.source_ref.as_deref().ok_or_else(denied)?)
                        .map_err(|_| denied())?;
                if parent.origins.is_empty() || parent.origins.len() > 256 {
                    return Err(denied());
                }
            }
        }
        Ok(())
    }

    pub fn values(&self, section: &str) -> Result<&[PreparedSecretValue]> {
        match section {
            EVENT_SOURCE_PAYLOAD => Ok(&self.payload),
            EVENT_SOURCE_CONFIG => Ok(&self.config),
            _ => Err(denied()),
        }
    }

    pub fn parent(&self, section: &str, path: &str) -> Result<SecretProvenance> {
        let row = self
            .values(section)?
            .iter()
            .find(|row| row.json_path == path)
            .ok_or_else(denied)?;
        serde_json::from_str(row.source_ref.as_deref().ok_or_else(denied)?).map_err(|_| denied())
    }

    pub fn restore_payload(
        &self,
        value: serde_json::Value,
        key: &str,
    ) -> Result<serde_json::Value> {
        let values = self
            .payload
            .iter()
            .map(|row| StoredSecretValue {
                json_path: row.json_path.clone(),
                source_kind: row.source_kind.clone(),
                source_ref: row.source_ref.clone(),
                encrypted_value: row.encrypted_value.clone(),
                encryption_key_hash: Some(row.encryption_key_hash.clone()),
            })
            .collect::<Vec<_>>();
        let restored = crate::secret_values::restore_secret_values(value, &values, key)?;
        if let Some(contract) = &self.binding.payload {
            crate::value_schema::CompiledContract::new(contract)
                .and_then(|contract| contract.validate(&restored))
                .map_err(|_| denied())?;
        }
        Ok(restored)
    }
}

pub struct EventOriginRepository;
fn denied() -> Error {
    Error::PermissionDenied("Exact event source is unavailable".into())
}
fn corrupt() -> Error {
    Error::validation("Invalid immutable event source evidence")
}

/// Private proof storage from actual exact event/trigger/pack rows. This is not
/// an IAM grant, and cannot be constructed from a retained display ref.
pub struct VerifiedEventSourceScope {
    context: crate::rbac::AuthorizationContext,
}
impl VerifiedEventSourceScope {
    pub fn allows(
        &self,
        caller: &crate::rbac::AuthorizationContext,
        grants: &[crate::rbac::Grant],
    ) -> bool {
        let mut context = self.context.clone();
        context.identity_id = caller.identity_id;
        context.identity_attributes = caller.identity_attributes.clone();
        [crate::rbac::Action::Read, crate::rbac::Action::Decrypt]
            .into_iter()
            .all(|action| {
                grants
                    .iter()
                    .any(|grant| grant.allows(crate::rbac::Resource::Events, action, &context))
            })
    }
}

impl EventOriginRepository {
    /// Run after partition move/drop commits, never as a physical row trigger.
    /// A surviving partition twin cannot retain another event's encrypted data.
    pub async fn cleanup_orphans(
        connection: &mut PgConnection,
        limit: i64,
        cutoff: DateTime<Utc>,
    ) -> Result<u64> {
        if limit <= 0 || limit > 10000 {
            return Err(denied());
        }
        let removed: i64 = sqlx::query_scalar(
            "WITH candidates AS MATERIALIZED (SELECT s.id,s.event,s.event_created FROM event_origin_snapshot s WHERE s.event_created < $2 \
             AND NOT EXISTS(SELECT 1 FROM event e WHERE e.id=s.event AND e.created=s.event_created) ORDER BY s.event_created,s.id LIMIT $1), \
             removed AS (DELETE FROM event_origin_snapshot s USING candidates c WHERE s.id=c.id \
             AND NOT EXISTS(SELECT 1 FROM event e WHERE e.id=c.event AND e.created=c.event_created) RETURNING s.id) SELECT count(*) FROM removed",
        ).bind(limit).bind(cutoff).fetch_one(connection).await?;
        Ok(removed as u64)
    }

    /// Producer owns the transaction through event insertion and publication.
    /// Legacy ID-only projections are emitted only while that ID is unambiguous.
    pub async fn capture(
        connection: &mut PgConnection,
        event: &Event,
        binding: &EventSchemaBinding,
        payload: &[PreparedSecretValue],
        config: &[PreparedSecretValue],
    ) -> Result<Id> {
        super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('event_origin'),hashtext($1::TEXT))")
            .bind(event.id.to_string())
            .execute(&mut *connection)
            .await?;
        let owner = super::event::EventRepository::find_by_identity(
            &mut *connection,
            event.id,
            event.created,
        )
        .await?
        .ok_or_else(denied)?;
        let managing_pack = if let Some(trigger) = event.trigger {
            sqlx::query_scalar::<_, Option<Id>>("SELECT CASE WHEN management_origin = 'pack' THEN pack END FROM trigger WHERE id = $1 FOR SHARE")
                .bind(trigger).fetch_optional(&mut *connection).await?.ok_or_else(denied)?
        } else {
            None
        };
        let snapshot = EventOriginSnapshot {
            event: event.id,
            created: event.created,
            trigger: event.trigger,
            managing_pack,
            trigger_ref: event.trigger_ref.clone(),
            binding: binding.clone(),
            payload: payload.to_vec(),
            config: config.to_vec(),
        };
        snapshot.validate()?;
        if !snapshot.matches(&owner) {
            return Err(denied());
        }
        let body = serde_json::to_value(&snapshot)?;
        let evidence = serde_json::to_value(snapshot.evidence()?)?;
        let evidence_hash = digest(&evidence).map_err(|_| denied())?;
        let hash = digest(&body).map_err(|_| denied())?;
        let size: i64 = sqlx::query_scalar("SELECT octet_length($1::JSONB::TEXT)::BIGINT")
            .bind(&body)
            .fetch_one(&mut *connection)
            .await?;
        if size > MAX_SNAPSHOT_BYTES as i64 {
            return Err(denied());
        }
        sqlx::query("INSERT INTO event_origin_snapshot(event,event_created,snapshot,snapshot_digest,evidence,evidence_digest) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (event,event_created) DO NOTHING")
            .bind(event.id).bind(event.created).bind(body).bind(&hash).bind(evidence).bind(evidence_hash).execute(&mut *connection).await?;
        let (id, stored): (Id, String) = sqlx::query_as("SELECT id,snapshot_digest FROM event_origin_snapshot WHERE event = $1 AND event_created = $2 FOR SHARE")
            .bind(event.id).bind(event.created).fetch_one(&mut *connection).await?;
        if stored != hash {
            return Err(denied());
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM (SELECT 1 FROM event WHERE id = $1 LIMIT 2) e",
        )
        .bind(event.id)
        .fetch_one(&mut *connection)
        .await?;
        if count == 1 {
            super::event_schema::EventSchemaRepository::attach(connection, event.id, binding)
                .await?;
            for (entity, values) in [
                (crate::secret_values::ENTITY_EVENT_PAYLOAD, payload),
                (crate::secret_values::ENTITY_EVENT_CONFIG, config),
            ] {
                super::execution_secret_value::ExecutionSecretValueRepository::upsert_many_with_conn(connection, entity, event.id, values).await?;
            }
        }
        Ok(id)
    }

    pub async fn find<'e, E: Executor<'e, Database = Postgres>>(
        executor: E,
        source: Id,
    ) -> Result<EventOriginSnapshot> {
        let (event, created, body, hash): (Id, DateTime<Utc>, serde_json::Value, String) = sqlx::query_as("SELECT event,event_created,snapshot,snapshot_digest FROM event_origin_snapshot WHERE id = $1")
            .bind(source).fetch_optional(executor).await?.ok_or_else(denied)?;
        if digest(&body).map_err(|_| denied())? != hash {
            return Err(denied());
        }
        let snapshot: EventOriginSnapshot = serde_json::from_value(body).map_err(|_| denied())?;
        snapshot.validate()?;
        if snapshot.event != event || snapshot.created != created {
            return Err(denied());
        }
        Ok(snapshot)
    }

    pub async fn source_id<'e, E: Executor<'e, Database = Postgres>>(
        executor: E,
        event: Id,
        created: DateTime<Utc>,
    ) -> Result<Id> {
        sqlx::query_scalar(
            "SELECT id FROM event_origin_snapshot WHERE event = $1 AND event_created = $2",
        )
        .bind(event)
        .bind(created)
        .fetch_optional(executor)
        .await?
        .ok_or_else(denied)
    }

    /// Disclosure projects no ciphertext. The evidence has an independent
    /// digest and a strict aggregate byte bound captured by the same producer.
    pub async fn evidence<'e, E: Executor<'e, Database = Postgres>>(
        executor: E,
        source: Id,
    ) -> Result<EventOriginEvidence> {
        let (event, created, body, hash): (Id, DateTime<Utc>, serde_json::Value, String) = sqlx::query_as("SELECT event,event_created,evidence,evidence_digest FROM event_origin_snapshot WHERE id = $1")
            .bind(source).fetch_optional(executor).await?.ok_or_else(denied)?;
        if digest(&body).map_err(|_| corrupt())? != hash {
            return Err(corrupt());
        }
        let evidence: EventOriginEvidence = serde_json::from_value(body).map_err(|_| corrupt())?;
        evidence.validate().map_err(|_| corrupt())?;
        if evidence.event != event || evidence.created != created {
            return Err(corrupt());
        }
        Ok(evidence)
    }

    pub async fn evidence_many<'e, E: Executor<'e, Database = Postgres>>(
        executor: E,
        sources: &[Id],
    ) -> Result<Vec<(Id, EventOriginEvidence)>> {
        if sources.len() > 256 {
            return Err(denied());
        }
        let rows: Vec<(Id, Id, DateTime<Utc>, Option<serde_json::Value>, String)> = sqlx::query_as(
            "SELECT id,event,event_created,CASE WHEN SUM(octet_length(evidence::TEXT)) OVER (ORDER BY id) <= 1048576 THEN evidence ELSE NULL END,evidence_digest FROM event_origin_snapshot WHERE id = ANY($1) ORDER BY id LIMIT 257")
            .bind(sources).fetch_all(executor).await?;
        let mut result = Vec::new();
        for (id, event, created, body, hash) in rows {
            let body = body.ok_or_else(denied)?;
            if digest(&body).map_err(|_| corrupt())? != hash {
                return Err(corrupt());
            }
            let evidence: EventOriginEvidence =
                serde_json::from_value(body).map_err(|_| corrupt())?;
            evidence.validate().map_err(|_| corrupt())?;
            if evidence.event != event || evidence.created != created {
                return Err(corrupt());
            }
            result.push((id, evidence));
        }
        Ok(result)
    }

    /// Batched ordinary-human source context proof. Recorded authorities must
    /// still use opaque runtime targets and their concrete policy pins.
    pub async fn human_scopes(
        connection: &mut PgConnection,
        sources: &[(Id, EventOriginEvidence)],
        identity: Id,
    ) -> Result<std::collections::HashMap<Id, VerifiedEventSourceScope>> {
        if sources.len() > 256 || identity <= 0 {
            return Err(denied());
        }
        let ids = sources.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let events = sources
            .iter()
            .map(|(_, evidence)| evidence.event)
            .collect::<Vec<_>>();
        let created = sources
            .iter()
            .map(|(_, evidence)| evidence.created)
            .collect::<Vec<_>>();
        let rows: Vec<(Id, Id, DateTime<Utc>, Option<Id>, String, Option<Id>, Option<String>)> = sqlx::query_as(
            "SELECT q.source,e.id,e.created,e.trigger,e.trigger_ref,p.id,p.ref FROM unnest($1::BIGINT[],$2::BIGINT[],$3::TIMESTAMPTZ[]) q(source,event,created) \
             JOIN event e ON e.id = q.event AND e.created = q.created \
             LEFT JOIN trigger t ON t.id = e.trigger LEFT JOIN pack p ON p.id = t.pack AND t.management_origin = 'pack' FOR SHARE OF e")
            .bind(ids).bind(events).bind(created).fetch_all(connection).await?;
        let expected = sources
            .iter()
            .map(|(id, source)| (*id, source))
            .collect::<std::collections::HashMap<_, _>>();
        let mut result = std::collections::HashMap::new();
        for (source, event, created, trigger, reference, pack, pack_ref) in rows {
            let original = expected.get(&source).ok_or_else(denied)?;
            if original.event != event
                || original.created != created
                || original.trigger != trigger
                || original.trigger_ref != reference
                || original.managing_pack != pack
            {
                continue;
            }
            let mut context = crate::rbac::AuthorizationContext::new(identity);
            context.target_id = Some(event);
            context.target_ref = Some(reference);
            context.pack_ref = pack_ref;
            context.managing_pack_id = pack;
            result.insert(source, VerifiedEventSourceScope { context });
        }
        Ok(result)
    }
}
