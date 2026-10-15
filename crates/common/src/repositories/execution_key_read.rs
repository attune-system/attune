use super::runtime_authority::ValidatedRuntimeAuthority;
use crate::{models::Id, secret_provenance::KeyOrigin, value_schema::digest, Error, Result};
use sqlx::{Executor, PgConnection, Postgres};

pub struct ExecutionKeyReadRepository;

impl ExecutionKeyReadRepository {
    pub async fn origins_for_executions<'e, E>(
        executor: E,
        executions: &[Id],
    ) -> Result<std::collections::BTreeMap<Id, Vec<KeyOrigin>>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let rows: Vec<(Id, serde_json::Value, String)> = sqlx::query_as(
            "SELECT a.owner_id, o.origin, o.origin_digest FROM execution_key_read_origin o \
                JOIN runtime_authority_admission a ON a.id = o.admission \
                WHERE a.owner_kind = 'execution' AND a.owner_id = ANY($1) \
                ORDER BY a.owner_id, o.origin_digest",
        )
        .bind(executions)
        .fetch_all(executor)
        .await?;
        let mut origins = std::collections::BTreeMap::<Id, Vec<KeyOrigin>>::new();
        for (execution, body, hash) in rows {
            if digest(&body).map_err(|_| Error::validation("Invalid runtime key origin"))? != hash {
                return Err(Error::validation("Runtime key origin digest mismatch"));
            }
            origins
                .entry(execution)
                .or_default()
                .push(serde_json::from_value(body)?);
        }
        Ok(origins)
    }

    /// Persist before returning plaintext, while the caller holds the validated
    /// admission and key-row locks through the protected read transaction.
    pub async fn record(
        connection: &mut PgConnection,
        authority: &ValidatedRuntimeAuthority,
        origin: &KeyOrigin,
    ) -> Result<()> {
        let admission = authority.credential_binding().admission_id;
        let body = serde_json::to_value(origin)?;
        let bytes: i64 = sqlx::query_scalar("SELECT octet_length($1::JSONB::TEXT)::BIGINT")
            .bind(&body)
            .fetch_one(&mut *connection)
            .await?;
        if bytes > 16384 {
            return Err(Error::validation(
                "Runtime key origin exceeds its byte limit",
            ));
        }
        let hash = digest(&body).map_err(|_| Error::validation("Invalid runtime key origin"))?;
        sqlx::query("INSERT INTO execution_key_read_origin_fence (admission) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(admission).execute(&mut *connection).await?;
        sqlx::query_scalar::<_, Id>(
            "SELECT admission FROM execution_key_read_origin_fence WHERE admission = $1 FOR UPDATE",
        )
        .bind(admission)
        .fetch_one(&mut *connection)
        .await?;
        let repeated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_key_read_origin WHERE admission = $1 AND origin_digest = $2)")
            .bind(admission).bind(&hash).fetch_one(&mut *connection).await?;
        if repeated {
            return Ok(());
        }
        let (count, used): (i64, i64) = sqlx::query_as("SELECT count(*), COALESCE(sum(octet_length(origin::TEXT)),0)::BIGINT FROM execution_key_read_origin WHERE admission = $1")
            .bind(admission).fetch_one(&mut *connection).await?;
        if count >= 1024 || used + bytes > 1024 * 1024 {
            return Err(Error::validation(
                "Runtime key read origins exceed their count or byte limit",
            ));
        }
        sqlx::query("INSERT INTO execution_key_read_origin (admission,origin_digest,origin) VALUES ($1,$2,$3)")
            .bind(admission).bind(hash).bind(body).execute(connection).await?;
        Ok(())
    }

    pub async fn origins(connection: &mut PgConnection, admission: Id) -> Result<Vec<KeyOrigin>> {
        let rows: Vec<(serde_json::Value, String)> = sqlx::query_as("SELECT origin,origin_digest FROM execution_key_read_origin WHERE admission = $1 ORDER BY origin_digest")
            .bind(admission).fetch_all(connection).await?;
        rows.into_iter()
            .map(|(body, hash)| {
                if digest(&body).map_err(|_| Error::validation("Invalid runtime key origin"))?
                    != hash
                {
                    return Err(Error::validation("Runtime key origin digest mismatch"));
                }
                serde_json::from_value(body).map_err(Into::into)
            })
            .collect()
    }
}
