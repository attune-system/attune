use serde_json::Value;
use sqlx::{Executor, PgConnection, Postgres};

use crate::{
    event_schema::{webhook_config_schema, EventSchemaBinding},
    models::Trigger,
    value_schema::{compile, digest, Limits, RegistryView, SchemaContext, SourceIdentity},
    Error, Result,
};

use super::{
    pack_release::PackReleaseRepository, schema_contract::SchemaContractRepository,
    schema_type::SchemaTypeRepository, trigger::TriggerRepository, FindById,
};

pub struct EventSchemaRepository;

impl EventSchemaRepository {
    /// Platform producers use the same transactional contract boundary as ingress.
    pub async fn create_internal(
        pool: &sqlx::PgPool,
        mut input: super::event::CreateEventInput,
        encryption_key: Option<&str>,
    ) -> Result<Option<crate::models::event::Event>> {
        use super::{Create, FindByRef};
        use crate::secret_values::{
            prepare_secret_values, ENTITY_EVENT_CONFIG, ENTITY_EVENT_PAYLOAD,
        };
        let mut tx = pool.begin().await?;
        SchemaTypeRepository::lock_selection(&mut *tx).await?;
        let Some(trigger) = TriggerRepository::find_by_ref(&mut *tx, &input.trigger_ref).await?
        else {
            return Ok(None);
        };
        if input.trigger.is_some_and(|id| id != trigger.id) {
            return Err(Error::validation(
                "Internal event trigger identity mismatch",
            ));
        }
        let (_, binding) = Self::materialize_current(&mut tx, trigger.id, false).await?;
        let sections = binding.prepare(input.payload, input.config)?;
        let prepare = |secrets: Vec<crate::secret_values::SecretValueInput>| -> Result<_> {
            if secrets.is_empty() {
                Ok(Vec::new())
            } else {
                prepare_secret_values(
                    secrets,
                    encryption_key.ok_or_else(|| {
                        Error::validation("Secret internal event values require encryption")
                    })?,
                )
            }
        };
        let payload_secrets = prepare(sections.payload_secrets)?;
        let config_secrets = prepare(sections.config_secrets)?;
        input.trigger = Some(trigger.id);
        input.payload = sections.payload;
        input.config = sections.config;
        let event = super::event::EventRepository::create(&mut *tx, input).await?;
        Self::attach(&mut tx, event.id, &binding).await?;
        for (entity, secrets) in [
            (ENTITY_EVENT_PAYLOAD, payload_secrets),
            (ENTITY_EVENT_CONFIG, config_secrets),
        ] {
            super::execution_secret_value::ExecutionSecretValueRepository::upsert_many_with_conn(
                &mut tx, entity, event.id, &secrets,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(Some(event))
    }
    /// Select under shared graph admission before any event/workload owner lock.
    pub async fn materialize_current(
        connection: &mut PgConnection,
        trigger_id: i64,
        webhook: bool,
    ) -> Result<(Trigger, EventSchemaBinding)> {
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        sqlx::query("SELECT id FROM trigger WHERE id = $1 FOR SHARE")
            .bind(trigger_id)
            .fetch_optional(&mut *connection)
            .await?;
        let trigger = TriggerRepository::find_by_id(&mut *connection, trigger_id)
            .await?
            .ok_or_else(|| Error::validation("Event trigger is unavailable"))?;
        if !trigger.enabled {
            return Err(Error::validation("Event trigger is disabled"));
        }
        let release_id: Option<i64> = sqlx::query_scalar(
            "SELECT COALESCE(t.managed_release, p.active_release) FROM trigger t \
             LEFT JOIN pack p ON p.id = t.pack WHERE t.id = $1",
        )
        .bind(trigger_id)
        .fetch_one(&mut *connection)
        .await?;
        let root = if let Some(id) = release_id {
            Some(
                PackReleaseRepository::find_by_id(&mut *connection, id)
                    .await?
                    .ok_or_else(|| {
                        Error::validation("Event schema source release is unavailable")
                    })?,
            )
        } else {
            None
        };
        let source = SourceIdentity::Assigned {
            component_ref: trigger.r#ref.clone(),
            assignment_id: uuid::Uuid::new_v4(),
        };
        let mut contracts = [None, None];
        for (slot, schema) in contracts.iter_mut().zip([
            trigger.out_schema.as_ref(),
            if webhook {
                None
            } else {
                trigger.event_config_schema.as_ref()
            },
        ]) {
            *slot = if let Some(schema) = schema {
                Some(if let Some(root) = &root {
                    if trigger.pack_ref.as_ref() != Some(&root.pack_ref) {
                        return Err(Error::validation(
                            "Event schema source does not match its pack",
                        ));
                    }
                    SchemaContractRepository::compile_selected(
                        connection,
                        source.clone(),
                        SchemaContext::FlatDocument,
                        schema,
                        Some(root.clone()),
                    )
                    .await?
                } else if let Some(pack) = trigger.pack {
                    let pack =
                        super::pack::PackRepository::get_by_id(&mut *connection, pack).await?;
                    SchemaContractRepository::compile_owned_assignment(
                        connection,
                        source.clone(),
                        SchemaContext::FlatDocument,
                        schema,
                        &pack,
                    )
                    .await?
                } else {
                    SchemaContractRepository::materialize_unowned(
                        connection,
                        source.clone(),
                        SchemaContext::FlatDocument,
                        schema,
                    )
                    .await?
                })
            } else {
                None
            };
        }
        let [payload, config] = contracts;
        let config = if webhook {
            Some(
                compile(
                    SourceIdentity::Assigned {
                        component_ref: "system.webhook-event-config".into(),
                        assignment_id: uuid::Uuid::new_v4(),
                    },
                    SchemaContext::FlatDocument,
                    &webhook_config_schema(),
                    &RegistryView::new(),
                    Limits::default(),
                )
                .map_err(|_| Error::validation("Invalid server webhook metadata schema"))?,
            )
        } else {
            config
        };
        let binding = EventSchemaBinding {
            trigger_id: Some(trigger.id),
            trigger_ref: trigger.r#ref.clone(),
            payload,
            config,
            webhook,
        };
        binding.validate()?;
        Ok((trigger, binding))
    }

    pub async fn attach(
        connection: &mut PgConnection,
        event_id: i64,
        binding: &EventSchemaBinding,
    ) -> Result<()> {
        binding.validate()?;
        let owner: Option<(Option<i64>, String, Option<String>)> = sqlx::query_as(
            "SELECT trigger, trigger_ref, source_ref FROM event WHERE id = $1 FOR KEY SHARE",
        )
        .bind(event_id)
        .fetch_optional(&mut *connection)
        .await?;
        if owner.as_ref().is_none_or(|(id, reference, source)| {
            *id != binding.trigger_id
                || *reference != binding.trigger_ref
                || (binding.webhook && source.as_deref() != Some("webhook"))
        }) {
            return Err(Error::validation("Event binding does not match its owner"));
        }
        let body = serde_json::to_value(binding)
            .map_err(|_| Error::validation("Invalid event binding"))?;
        let hash = digest(&body).map_err(|_| Error::validation("Invalid event binding digest"))?;
        sqlx::query("INSERT INTO event_schema_binding (event, contract, binding_digest) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(event_id).bind(body).bind(&hash).execute(&mut *connection).await?;
        let stored: String =
            sqlx::query_scalar("SELECT binding_digest FROM event_schema_binding WHERE event = $1")
                .bind(event_id)
                .fetch_one(&mut *connection)
                .await?;
        if stored != hash {
            return Err(Error::validation(
                "Event already has another immutable binding",
            ));
        }
        Ok(())
    }

    /// Partition repair must not run owner-delete triggers. Prune only owners
    /// absent from the partitioned parent after the complete move/drop commits.
    pub async fn cleanup_orphans(
        connection: &mut PgConnection,
        limit: i64,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64> {
        if limit <= 0 || limit == i64::MAX {
            return Err(Error::validation(
                "Event cleanup requires a bounded positive limit",
            ));
        }
        let removed = sqlx::query_scalar::<_, i64>(
            "WITH candidates AS MATERIALIZED (SELECT b.event FROM event_schema_binding b \
                 WHERE b.created < $2 ORDER BY b.created, b.event LIMIT $1), \
             removed AS (DELETE FROM event_schema_binding b USING candidates c \
                 WHERE b.event = c.event AND NOT EXISTS (SELECT 1 FROM event e WHERE e.id = b.event) RETURNING b.event), \
             secrets AS (DELETE FROM execution_secret_value s USING removed r \
                 WHERE s.entity_type IN ('event_payload', 'event_config') AND s.entity_id = r.event) \
             SELECT COUNT(*) FROM removed"
         ).bind(limit).bind(cutoff).fetch_one(&mut *connection).await?;
        super::execution_event_emitter::ExecutionEventEmitterRepository::cleanup_orphans(
            connection,
            limit.min(10000),
            cutoff,
        )
        .await?;
        Ok(removed as u64)
    }

    pub async fn find<'e, E>(executor: E, event_id: i64) -> Result<Option<EventSchemaBinding>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let row: Option<(Value, String)> = sqlx::query_as(
            "SELECT contract, binding_digest FROM event_schema_binding WHERE event = $1",
        )
        .bind(event_id)
        .fetch_optional(executor)
        .await?;
        let Some((body, hash)) = row else {
            return Ok(None);
        };
        Self::decode_binding(body, &hash).map(Some)
    }

    pub async fn find_many<'e, E>(
        executor: E,
        ids: &[i64],
    ) -> Result<Vec<(i64, EventSchemaBinding)>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let rows: Vec<(i64, Value, String)> = sqlx::query_as("SELECT event, contract, binding_digest FROM event_schema_binding WHERE event = ANY($1)")
            .bind(ids).fetch_all(executor).await?;
        rows.into_iter()
            .map(|(id, body, hash)| Self::decode_binding(body, &hash).map(|binding| (id, binding)))
            .collect()
    }

    fn decode_binding(body: Value, hash: &str) -> Result<EventSchemaBinding> {
        if digest(&body).map_err(|_| Error::validation("Invalid stored event digest"))? != hash {
            return Err(Error::validation("Stored event binding digest mismatch"));
        }
        let binding: EventSchemaBinding = serde_json::from_value(body)
            .map_err(|_| Error::validation("Invalid stored event binding"))?;
        binding.validate()?;
        Ok(binding)
    }
}
