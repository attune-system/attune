use chrono::{DateTime, Utc};
use sqlx::{Executor, PgConnection, Postgres};

use crate::models::{identity::IntegrationToken, Id};
use crate::Result;

use super::{Create, Delete, FindById, Repository};

const SELECT_COLUMNS: &str = "id, identity, label, description, token_hash, token_prefix, token_suffix, created_by, expires_at, last_used_at, last_used_ip, revoked_at, revoked_by, revocation_reason, created, updated";

pub struct IntegrationTokenRepository;

impl Repository for IntegrationTokenRepository {
    type Entity = IntegrationToken;

    fn table_name() -> &'static str {
        "integration_token"
    }
}

#[derive(Debug, Clone)]
pub struct CreateIntegrationTokenInput {
    pub identity: Id,
    pub label: String,
    pub description: Option<String>,
    pub token_hash: String,
    pub token_prefix: String,
    pub token_suffix: String,
    pub created_by: Option<Id>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[async_trait::async_trait]
impl Create for IntegrationTokenRepository {
    type CreateInput = CreateIntegrationTokenInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IntegrationToken>(&format!(
            "INSERT INTO integration_token (identity, label, description, token_hash, token_prefix, token_suffix, created_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING {SELECT_COLUMNS}"
        ))
        .bind(input.identity)
        .bind(&input.label)
        .bind(&input.description)
        .bind(&input.token_hash)
        .bind(&input.token_prefix)
        .bind(&input.token_suffix)
        .bind(input.created_by)
        .bind(input.expires_at)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl FindById for IntegrationTokenRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IntegrationToken>(&format!(
            "SELECT {SELECT_COLUMNS} FROM integration_token WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for IntegrationTokenRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM integration_token WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl IntegrationTokenRepository {
    /// Locks and returns an active credential only when it belongs to the
    /// expected, unfrozen integration identity.
    pub async fn find_active_for_identity(
        conn: &mut PgConnection,
        id: Id,
        identity: Id,
    ) -> Result<Option<IntegrationToken>> {
        sqlx::query_as::<_, IntegrationToken>(&format!(
            "SELECT {} FROM integration_token t \
             JOIN identity i ON i.id = t.identity \
             WHERE t.id = $1 AND t.identity = $2 AND t.revoked_at IS NULL \
               AND (t.expires_at IS NULL OR t.expires_at > NOW()) AND NOT i.frozen \
             FOR SHARE OF t, i",
            SELECT_COLUMNS
                .split(", ")
                .map(|column| format!("t.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .bind(id)
        .bind(identity)
        .fetch_optional(conn)
        .await
        .map_err(Into::into)
    }

    pub async fn list_by_identity<'e, E>(executor: E, identity: Id) -> Result<Vec<IntegrationToken>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IntegrationToken>(&format!(
            "SELECT {SELECT_COLUMNS}
             FROM integration_token
             WHERE identity = $1
             ORDER BY created DESC"
        ))
        .bind(identity)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_hash<'e, E>(
        executor: E,
        token_hash: &str,
    ) -> Result<Option<IntegrationToken>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IntegrationToken>(&format!(
            "SELECT {SELECT_COLUMNS} FROM integration_token WHERE token_hash = $1"
        ))
        .bind(token_hash)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn touch_last_used<'e, E>(
        executor: E,
        id: Id,
        last_used_ip: Option<&str>,
    ) -> Result<IntegrationToken>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IntegrationToken>(&format!(
            "UPDATE integration_token
             SET last_used_at = NOW(), last_used_ip = $2
             WHERE id = $1
             RETURNING {SELECT_COLUMNS}"
        ))
        .bind(id)
        .bind(last_used_ip)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn revoke<'e, E>(
        executor: E,
        id: Id,
        revoked_by: Option<Id>,
        reason: Option<&str>,
    ) -> Result<IntegrationToken>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, IntegrationToken>(&format!(
            "UPDATE integration_token
             SET revoked_at = COALESCE(revoked_at, NOW()),
                 revoked_by = COALESCE(revoked_by, $2),
                 revocation_reason = COALESCE(revocation_reason, $3)
             WHERE id = $1
             RETURNING {SELECT_COLUMNS}"
        ))
        .bind(id)
        .bind(revoked_by)
        .bind(reason)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }
}
