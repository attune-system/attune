use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::Result;

#[derive(Debug, Clone, FromRow)]
pub struct LegacyPackFile {
    pub id: i64,
    pub archive_path: String,
    pub digest: String,
    pub archive_size: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct LegacyArtifactFile {
    pub id: i64,
    pub artifact: i64,
    pub version: i32,
    pub file_path: String,
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ObjectBodyCandidate {
    pub id: i64,
    pub artifact: i64,
    pub object_key: String,
    pub provider_version: Option<String>,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    pub file_path: Option<String>,
    pub legacy_snapshot_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct SharedLogBodyCandidate {
    pub id: i64,
    pub artifact: i64,
    pub file_path: String,
}

pub struct StorageMaintenanceRepository;

impl StorageMaintenanceRepository {
    pub async fn legacy_pack_files(pool: &PgPool) -> Result<Vec<LegacyPackFile>> {
        sqlx::query_as(
            "SELECT id, archive_path, digest, archive_size FROM pack_release \
             WHERE object_key IS NULL AND archive_path IS NOT NULL ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn legacy_artifact_files(pool: &PgPool) -> Result<Vec<LegacyArtifactFile>> {
        sqlx::query_as(
            "SELECT id, artifact, version, file_path, size_bytes FROM artifact_version \
             WHERE body_state IS NULL AND file_path IS NOT NULL ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn switch_pack_file(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        object_key: &str,
        provider_version: &str,
        snapshot_expires_at: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE pack_release SET object_key = $2, provider_version = $3, \
             legacy_snapshot_expires_at = $4 WHERE id = $1 AND object_key IS NULL",
        )
        .bind(id)
        .bind(object_key)
        .bind(provider_version)
        .bind(snapshot_expires_at)
        .execute(&mut **tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn switch_artifact_file(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        object_key: &str,
        provider_version: &str,
        size_bytes: i64,
        sha256: &str,
        snapshot_expires_at: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE artifact_version SET body_state = 'ready', object_key = $2, \
             provider_version = $3, size_bytes = $4, sha256 = $5, \
             legacy_snapshot_expires_at = $6 WHERE id = $1 AND body_state IS NULL",
        )
        .bind(id)
        .bind(object_key)
        .bind(provider_version)
        .bind(size_bytes)
        .bind(sha256)
        .bind(snapshot_expires_at)
        .execute(&mut **tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn abandoned_pending(
        pool: &PgPool,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ObjectBodyCandidate>> {
        Self::body_candidates(pool, "av.object_key IS NOT NULL AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g WHERE g.artifact_version = av.id AND g.state = 'issued' AND g.settle_until > clock_timestamp()) AND (av.body_state = 'cleanup_claimed' OR (av.body_state = 'pending' AND av.body_updated < $1 AND (NOT EXISTS (SELECT 1 FROM log_stream ls WHERE ls.artifact_version = av.id) OR EXISTS (SELECT 1 FROM log_stream ls JOIN execution e ON e.id = av.execution WHERE ls.artifact_version = av.id AND NOT ls.sealed AND e.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')))))", cutoff, limit).await
    }

    pub async fn abandoned_shared_log_pending(
        pool: &PgPool,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<SharedLogBodyCandidate>> {
        sqlx::query_as(
            "SELECT av.id, av.artifact, av.file_path FROM artifact_version av \
             WHERE av.object_key IS NULL AND av.file_path IS NOT NULL AND ( \
              av.body_state = 'cleanup_claimed' OR (av.body_state = 'pending' \
              AND av.body_updated < $1 AND EXISTS ( \
                  SELECT 1 FROM log_stream ls WHERE ls.artifact_version = av.id \
                  AND ls.backend = 'shared_file' AND NOT ls.sealed \
              ) AND EXISTS ( \
                  SELECT 1 FROM execution e WHERE e.id = av.execution \
                  AND e.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned') \
              ))) ORDER BY av.body_updated, av.id LIMIT $2",
        )
        .bind(cutoff)
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn claim_abandoned_pending(
        pool: &PgPool,
        id: i64,
        cutoff: DateTime<Utc>,
    ) -> Result<bool> {
        Self::claim_pending(pool, id, cutoff, false).await
    }

    pub async fn claim_abandoned_shared_log_pending(
        pool: &PgPool,
        id: i64,
        cutoff: DateTime<Utc>,
    ) -> Result<bool> {
        Self::claim_pending(pool, id, cutoff, true).await
    }

    async fn claim_pending(
        pool: &PgPool,
        id: i64,
        cutoff: DateTime<Utc>,
        shared_file: bool,
    ) -> Result<bool> {
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT id FROM artifact_version WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('log_stream'), hashtext($1::text))")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT id FROM log_stream WHERE artifact_version = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let storage_predicate = if shared_file {
            "av.object_key IS NULL AND av.file_path IS NOT NULL AND EXISTS (SELECT 1 FROM log_stream ls WHERE ls.artifact_version = av.id AND ls.backend = 'shared_file' AND NOT ls.sealed)"
        } else {
            "av.object_key IS NOT NULL AND (NOT EXISTS (SELECT 1 FROM log_stream ls WHERE ls.artifact_version = av.id) OR EXISTS (SELECT 1 FROM log_stream ls WHERE ls.artifact_version = av.id AND NOT ls.sealed))"
        };
        let query = format!(
            "UPDATE artifact_version av SET body_state = 'cleanup_claimed' \
             WHERE av.id = $1 AND av.body_state = 'pending' AND av.body_updated < $2 \
             AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g \
                             WHERE g.artifact_version = av.id AND g.state = 'issued' \
                               AND g.settle_until > clock_timestamp()) \
             AND {storage_predicate} AND (NOT EXISTS (SELECT 1 FROM log_stream ls WHERE ls.artifact_version = av.id) \
             OR EXISTS (SELECT 1 FROM execution e WHERE e.id = av.execution AND e.status IN \
             ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')))"
        );
        let claimed = sqlx::query(&query)
            .bind(id)
            .bind(cutoff)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        let already_claimed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM artifact_version WHERE id = $1 AND body_state = 'cleanup_claimed')",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(claimed || already_claimed)
    }

    pub async fn ready_objects(
        pool: &PgPool,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<ObjectBodyCandidate>> {
        sqlx::query_as(
            "SELECT av.id, av.artifact, av.object_key, av.provider_version, av.size_bytes, \
                     av.sha256, av.file_path, av.legacy_snapshot_expires_at \
             FROM artifact_version av WHERE av.body_state = 'ready' AND av.object_key IS NOT NULL AND av.id > $1 \
             ORDER BY av.id LIMIT $2",
        )
        .bind(after_id.max(0))
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn deleting_objects(
        pool: &PgPool,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ObjectBodyCandidate>> {
        Self::body_candidates(
            pool,
            "av.body_state = 'deleting' AND av.object_key IS NOT NULL AND av.body_updated < $1",
            cutoff,
            limit,
        )
        .await
    }

    async fn body_candidates(
        pool: &PgPool,
        predicate: &str,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ObjectBodyCandidate>> {
        let query = format!(
            "SELECT av.id, av.artifact, av.object_key, av.provider_version, av.size_bytes, \
                    av.sha256, av.file_path, av.legacy_snapshot_expires_at \
             FROM artifact_version av WHERE {predicate} ORDER BY av.body_updated, av.id LIMIT $2"
        );
        sqlx::query_as(&query)
            .bind(cutoff)
            .bind(limit.max(1))
            .fetch_all(pool)
            .await
            .map_err(Into::into)
    }

    pub async fn mark_deleting_from_pending(
        pool: &PgPool,
        id: i64,
        provider_version: &str,
        size_bytes: i64,
        sha256: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE artifact_version av SET body_state = 'deleting', provider_version = $2, \
             size_bytes = $3, sha256 = $4 WHERE av.id = $1 AND av.body_state = 'pending' \
             AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g \
                             WHERE g.artifact_version = av.id AND g.state = 'issued' \
                               AND g.settle_until > clock_timestamp())",
        )
        .bind(id)
        .bind(provider_version)
        .bind(size_bytes)
        .bind(sha256)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_log_deleting_from_pending(
        pool: &PgPool,
        id: i64,
        segment_count: i64,
        size_bytes: i64,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE artifact_version SET body_state = 'deleting', provider_version = $2, \
             size_bytes = $3, sha256 = repeat('0', 64) \
             WHERE id = $1 AND body_state = 'pending'",
        )
        .bind(id)
        .bind(format!("segments:{segment_count}"))
        .bind(size_bytes)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_deleting(pool: &PgPool, id: i64) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE artifact_version SET body_state = 'deleting' \
             WHERE id = $1 AND body_state = 'ready'",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn delete_cleanup_claimed(pool: &PgPool, id: i64) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM artifact_version av \
             WHERE av.id = $1 AND av.body_state = 'cleanup_claimed' \
             AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g \
                             WHERE g.artifact_version = av.id AND g.state = 'issued' \
                               AND g.settle_until > clock_timestamp())",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn delete_deleting(pool: &PgPool, id: i64) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM artifact_version av WHERE av.id = $1 AND av.body_state = 'deleting' \
             AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g \
                             WHERE g.artifact_version = av.id AND g.state = 'issued' \
                               AND g.settle_until > clock_timestamp())",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn expired_pack_snapshots(pool: &PgPool, limit: i64) -> Result<Vec<(i64, String)>> {
        sqlx::query_as(
            "SELECT id, archive_path FROM pack_release WHERE archive_path IS NOT NULL \
             AND legacy_snapshot_expires_at <= NOW() ORDER BY id LIMIT $1",
        )
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn clear_pack_snapshot(pool: &PgPool, id: i64) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE pack_release SET archive_path = NULL, legacy_snapshot_expires_at = NULL \
             WHERE id = $1 AND legacy_snapshot_expires_at <= NOW()",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn expired_artifact_snapshots(
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<(i64, String)>> {
        sqlx::query_as(
            "SELECT id, file_path FROM artifact_version WHERE file_path IS NOT NULL \
             AND legacy_snapshot_expires_at <= NOW() ORDER BY id LIMIT $1",
        )
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn clear_artifact_snapshot(pool: &PgPool, id: i64) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE artifact_version SET file_path = NULL, legacy_snapshot_expires_at = NULL \
             WHERE id = $1 AND legacy_snapshot_expires_at <= NOW()",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}
