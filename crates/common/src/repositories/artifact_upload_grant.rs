use chrono::{DateTime, Utc};
use sqlx::{Executor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::models::artifact_upload_grant::{ArtifactUploadGrant, SELECT_COLUMNS};
use crate::Result;

#[derive(Debug, Clone)]
pub struct CreateArtifactUploadGrantInput {
    pub token: Uuid,
    pub artifact_version: i64,
    pub segment_sequence: Option<i64>,
    pub object_key: String,
    pub expected_size: i64,
    pub expected_sha256: String,
    pub content_type: String,
    pub expires_at: DateTime<Utc>,
    pub settle_until: DateTime<Utc>,
}

pub struct ArtifactUploadGrantRepository;

impl ArtifactUploadGrantRepository {
    pub async fn create<'e, E>(
        executor: E,
        input: CreateArtifactUploadGrantInput,
    ) -> Result<ArtifactUploadGrant>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "INSERT INTO artifact_upload_grant \
                 (token, artifact_version, segment_sequence, object_key, expected_size, expected_sha256, \
                  content_type, expires_at, settle_until) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {SELECT_COLUMNS}"
        );
        sqlx::query_as(&query)
            .bind(input.token)
            .bind(input.artifact_version)
            .bind(input.segment_sequence)
            .bind(input.object_key)
            .bind(input.expected_size)
            .bind(input.expected_sha256)
            .bind(input.content_type)
            .bind(input.expires_at)
            .bind(input.settle_until)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_artifact_version<'e, E>(
        executor: E,
        artifact_version: i64,
    ) -> Result<Option<ArtifactUploadGrant>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM artifact_upload_grant \
             WHERE artifact_version = $1 AND segment_sequence IS NULL"
        );
        sqlx::query_as(&query)
            .bind(artifact_version)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_log_segment<'e, E>(
        executor: E,
        artifact_version: i64,
        sequence: i64,
    ) -> Result<Option<ArtifactUploadGrant>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM artifact_upload_grant \
             WHERE artifact_version = $1 AND segment_sequence = $2"
        );
        sqlx::query_as(&query)
            .bind(artifact_version)
            .bind(sequence)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_token<'e, E>(
        executor: E,
        token: Uuid,
    ) -> Result<Option<ArtifactUploadGrant>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {SELECT_COLUMNS} FROM artifact_upload_grant WHERE token = $1");
        sqlx::query_as(&query)
            .bind(token)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_artifact_version_for_update(
        tx: &mut Transaction<'_, Postgres>,
        artifact_version: i64,
    ) -> Result<Option<ArtifactUploadGrant>> {
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM artifact_upload_grant \
             WHERE artifact_version = $1 AND segment_sequence IS NULL FOR UPDATE"
        );
        sqlx::query_as(&query)
            .bind(artifact_version)
            .fetch_optional(&mut **tx)
            .await
            .map_err(Into::into)
    }

    pub async fn find_log_segment_for_update(
        tx: &mut Transaction<'_, Postgres>,
        artifact_version: i64,
        sequence: i64,
    ) -> Result<Option<ArtifactUploadGrant>> {
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM artifact_upload_grant \
             WHERE artifact_version = $1 AND segment_sequence = $2 FOR UPDATE"
        );
        sqlx::query_as(&query)
            .bind(artifact_version)
            .bind(sequence)
            .fetch_optional(&mut **tx)
            .await
            .map_err(Into::into)
    }

    pub async fn active_log_segment_exists<'e, E>(
        executor: E,
        artifact_version: i64,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM artifact_upload_grant \
             WHERE artifact_version = $1 AND segment_sequence IS NOT NULL \
               AND state = 'issued' AND settle_until > clock_timestamp())",
        )
        .bind(artifact_version)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_token_for_update(
        tx: &mut Transaction<'_, Postgres>,
        token: Uuid,
    ) -> Result<Option<ArtifactUploadGrant>> {
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM artifact_upload_grant WHERE token = $1 FOR UPDATE"
        );
        sqlx::query_as(&query)
            .bind(token)
            .fetch_optional(&mut **tx)
            .await
            .map_err(Into::into)
    }

    pub async fn mark_completed(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        provider_version: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE artifact_upload_grant \
             SET state = 'completed', completed_provider_version = $2, \
                 completed_at = clock_timestamp() \
             WHERE id = $1 AND state = 'issued'",
        )
        .bind(id)
        .bind(provider_version)
        .execute(&mut **tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn renew(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        expires_at: DateTime<Utc>,
        settle_until: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE artifact_upload_grant \
             SET state = 'issued', expires_at = $2, settle_until = $3 \
             WHERE id = $1 AND state IN ('issued', 'expired') \
               AND settle_until <= clock_timestamp()",
        )
        .bind(id)
        .bind(expires_at)
        .bind(settle_until)
        .execute(&mut **tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_expired(pool: &PgPool, limit: i64) -> Result<u64> {
        let result = sqlx::query(
            "WITH expired AS ( \
                 SELECT id FROM artifact_upload_grant \
                 WHERE state = 'issued' AND settle_until <= clock_timestamp() \
                 ORDER BY settle_until, id FOR UPDATE SKIP LOCKED LIMIT $1 \
             ) \
             UPDATE artifact_upload_grant grant_row SET state = 'expired' \
             FROM expired WHERE grant_row.id = expired.id",
        )
        .bind(limit.max(1))
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn purge_terminal(pool: &PgPool, cutoff: DateTime<Utc>, limit: i64) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM artifact_upload_grant WHERE id IN ( \
                 SELECT id FROM artifact_upload_grant \
                 WHERE state IN ('completed', 'expired') AND updated < $1 \
                 ORDER BY updated, id FOR UPDATE SKIP LOCKED LIMIT $2 \
             )",
        )
        .bind(cutoff)
        .bind(limit.max(1))
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}
