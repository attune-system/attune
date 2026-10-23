use super::runtime_authority::ValidatedRuntimeAuthority;
use crate::{
    models::{Event, Id},
    runtime_authority::RequestAttribution,
    value_schema::digest,
    Error, Result,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEventEmitter {
    pub execution_id: Id,
    pub admission_id: Id,
    pub sponsor_identity_id: Id,
    pub pack_id: Id,
    pub pack_ref: String,
    pub action_ref: String,
    pub policy_revision_id: Id,
    pub release_id: Id,
    pub release_digest: String,
    pub attempt: uuid::Uuid,
    pub requester: RequestAttribution,
}

pub struct ExecutionEventEmitterRepository;

impl ExecutionEventEmitterRepository {
    pub async fn attach(
        connection: &mut PgConnection,
        event: &Event,
        authority: &ValidatedRuntimeAuthority,
    ) -> Result<ExecutionEventEmitter> {
        if event.source.is_some() || event.source_ref.is_some() {
            return Err(Error::validation(
                "Execution emission cannot claim a sensor source",
            ));
        }
        let credential = authority.credential_binding();
        let selected = authority.authority();
        let emitter = ExecutionEventEmitter {
            execution_id: credential.execution_id,
            admission_id: credential.admission_id,
            sponsor_identity_id: credential.sponsor_identity_id,
            pack_id: selected.owner.pack_id,
            pack_ref: selected.owner.pack_ref.clone(),
            action_ref: credential.action_ref,
            policy_revision_id: selected.policy_id,
            release_id: selected.target_release_id,
            release_digest: selected.target_release_digest.clone(),
            attempt: credential.attempt,
            requester: selected.attribution.clone(),
        };
        let value = serde_json::to_value(&emitter)?;
        let hash =
            digest(&value).map_err(|_| Error::validation("Invalid event emitter metadata"))?;
        sqlx::query(
            "INSERT INTO execution_event_emitter (event,event_created,emitter,emitter_digest) \
            VALUES ($1,$2,$3,$4) ON CONFLICT (event,event_created) DO NOTHING",
        )
        .bind(event.id)
        .bind(event.created)
        .bind(value)
        .bind(hash)
        .execute(&mut *connection)
        .await?;
        let stored = Self::find(connection, event.id, event.created)
            .await?
            .ok_or_else(|| Error::validation("Event emitter metadata is unavailable"))?;
        if stored != emitter {
            return Err(Error::validation(
                "Event emitter identity cannot be rebound",
            ));
        }
        Ok(stored)
    }

    pub async fn find(
        connection: &mut PgConnection,
        event: Id,
        created: DateTime<Utc>,
    ) -> Result<Option<ExecutionEventEmitter>> {
        let row: Option<(serde_json::Value, String)> = sqlx::query_as(
            "SELECT emitter, emitter_digest FROM execution_event_emitter WHERE event = $1 AND event_created = $2",
        ).bind(event).bind(created).fetch_optional(connection).await?;
        let Some((body, hash)) = row else {
            return Ok(None);
        };
        if digest(&body).map_err(|_| Error::validation("Invalid event emitter metadata"))? != hash {
            return Err(Error::validation("Event emitter metadata digest mismatch"));
        }
        Ok(Some(serde_json::from_value(body)?))
    }

    pub async fn cleanup_orphans(
        connection: &mut PgConnection,
        limit: i64,
        cutoff: DateTime<Utc>,
    ) -> Result<u64> {
        if !(1..=10000).contains(&limit) {
            return Err(Error::validation(
                "Emitter cleanup limit must be 1 to 10000",
            ));
        }
        let removed = sqlx::query("WITH candidates AS (SELECT event,event_created FROM execution_event_emitter \
            WHERE event_created < $2 ORDER BY event_created,event LIMIT $1 FOR UPDATE SKIP LOCKED) \
            DELETE FROM execution_event_emitter a USING candidates c \
            WHERE a.event = c.event AND a.event_created = c.event_created \
              AND NOT EXISTS (SELECT 1 FROM event e WHERE e.id = a.event AND e.created = a.event_created)")
            .bind(limit).bind(cutoff).execute(connection).await?;
        Ok(removed.rows_affected())
    }
}
