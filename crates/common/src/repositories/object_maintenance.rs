use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool, Postgres};

use crate::{Error, Result};

#[derive(Debug, Clone, FromRow)]
pub struct ObjectLedgerEntry {
    pub id: i64,
    pub object_key: String,
    pub provider_version: Option<String>,
    pub object_kind: String,
    pub size_bytes: Option<i64>,
    pub state: String,
    pub attempts: i32,
    pub updated: DateTime<Utc>,
}

pub struct ObjectMaintenanceRepository;

impl ObjectMaintenanceRepository {
    pub async fn reserve_upload(pool: &PgPool, key: &str, kind: &str) -> Result<()> {
        let reserved = sqlx::query_scalar::<_, i64>(
            "INSERT INTO object_maintenance_ledger (object_key, object_kind) VALUES ($1, $2) \
             ON CONFLICT (object_key) DO UPDATE SET \
                 state = CASE WHEN object_maintenance_ledger.state = 'deletion_pending' \
                              THEN 'ready' ELSE object_maintenance_ledger.state END, \
                 eligible_at = CASE WHEN object_maintenance_ledger.state = 'deletion_pending' \
                                    THEN NULL ELSE object_maintenance_ledger.eligible_at END, \
                 updated = NOW() \
             WHERE object_maintenance_ledger.state <> 'deleting' \
             RETURNING id",
        )
        .bind(key)
        .bind(kind)
        .fetch_optional(pool)
        .await?;
        if reserved.is_none() {
            return Err(Error::invalid_state(format!(
                "object '{key}' is being deleted"
            )));
        }
        Ok(())
    }

    pub async fn record_uploaded<'e, E>(
        executor: E,
        key: &str,
        provider_version: &str,
        size_bytes: i64,
    ) -> Result<()>
    where
        E: sqlx::Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            "UPDATE object_maintenance_ledger SET provider_version = $2, size_bytes = $3, \
             state = 'ready', eligible_at = NULL, updated = NOW(), last_error = NULL \
             WHERE object_key = $1 AND state IN ('uploading', 'ready') \
             AND (provider_version IS NULL OR provider_version = $2)",
        )
        .bind(key)
        .bind(provider_version)
        .bind(size_bytes)
        .execute(executor)
        .await?;
        if result.rows_affected() != 1 {
            return Err(Error::invalid_state(format!(
                "object '{key}' upload is no longer reserved"
            )));
        }
        Ok(())
    }

    pub async fn stale_uploads(
        pool: &PgPool,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ObjectLedgerEntry>> {
        sqlx::query_as(
            "SELECT id, object_key, provider_version, object_kind, size_bytes, state, attempts, updated \
             FROM object_maintenance_ledger WHERE state = 'uploading' AND updated < $1 \
             ORDER BY updated, id LIMIT $2",
        )
        .bind(cutoff)
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn remove_stale_upload(
        pool: &PgPool,
        id: i64,
        observed_updated: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM object_maintenance_ledger \
             WHERE id = $1 AND state = 'uploading' AND updated = $2",
        )
        .bind(id)
        .bind(observed_updated)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn schedule_unreferenced(
        pool: &PgPool,
        upload_cutoff: DateTime<Utc>,
        eligible_at: DateTime<Utc>,
    ) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE object_maintenance_ledger l SET state = 'deletion_pending', \
              eligible_at = $1, updated = NOW() WHERE l.state = 'ready' \
              AND l.updated < $2 \
              AND NOT EXISTS (SELECT 1 FROM pack_release r WHERE r.object_key = l.object_key AND r.provider_version = l.provider_version) \
              AND NOT EXISTS (SELECT 1 FROM artifact_version v WHERE v.object_key = l.object_key AND v.provider_version = l.provider_version) \
              AND NOT EXISTS (SELECT 1 FROM log_segment s WHERE s.object_key = l.object_key AND s.provider_version = l.provider_version)",
        )
        .bind(eligible_at)
        .bind(upload_cutoff)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn claim_deletions(
        pool: &PgPool,
        retry_cutoff: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ObjectLedgerEntry>> {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query_as(
            "WITH candidates AS ( \
                 SELECT id FROM object_maintenance_ledger \
                 WHERE (state = 'deleting' AND updated < $1) OR ( \
                   state = 'deletion_pending' AND eligible_at < $1 \
                   AND NOT EXISTS (SELECT 1 FROM pack_release r \
                                   WHERE r.object_key = object_maintenance_ledger.object_key \
                                     AND r.provider_version = object_maintenance_ledger.provider_version) \
                   AND NOT EXISTS (SELECT 1 FROM artifact_version v \
                                   WHERE v.object_key = object_maintenance_ledger.object_key \
                                     AND v.provider_version = object_maintenance_ledger.provider_version) \
                   AND NOT EXISTS (SELECT 1 FROM log_segment s \
                                   WHERE s.object_key = object_maintenance_ledger.object_key \
                                     AND s.provider_version = object_maintenance_ledger.provider_version)) \
                 ORDER BY eligible_at NULLS LAST, updated, id \
                 FOR UPDATE SKIP LOCKED LIMIT $2 \
             ) \
             UPDATE object_maintenance_ledger l SET state = 'deleting', attempts = attempts + 1, updated = NOW() \
             FROM candidates c WHERE l.id = c.id \
             RETURNING l.id, l.object_key, l.provider_version, l.object_kind, l.size_bytes, l.state, l.attempts, l.updated",
        )
        .bind(retry_cutoff)
        .bind(limit.max(1))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn exact_reference_exists(pool: &PgPool, key: &str, version: &str) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pack_release WHERE object_key = $1 AND provider_version = $2) \
             OR EXISTS (SELECT 1 FROM artifact_version WHERE object_key = $1 AND provider_version = $2) \
             OR EXISTS (SELECT 1 FROM log_segment WHERE object_key = $1 AND provider_version = $2)",
        )
        .bind(key)
        .bind(version)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn complete_delete(pool: &PgPool, id: i64) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM object_maintenance_ledger WHERE id = $1 AND state = 'deleting'",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn defer_referenced(pool: &PgPool, id: i64) -> Result<()> {
        sqlx::query("UPDATE object_maintenance_ledger SET state = 'ready', eligible_at = NULL, updated = NOW() WHERE id = $1")
            .bind(id).execute(pool).await?;
        Ok(())
    }

    pub async fn record_failure(pool: &PgPool, id: i64, error: &str) -> Result<()> {
        sqlx::query("UPDATE object_maintenance_ledger SET last_error = LEFT($2, 1000), updated = NOW() WHERE id = $1")
            .bind(id).bind(error).execute(pool).await?;
        Ok(())
    }

    pub async fn pending_count(pool: &PgPool) -> Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM object_maintenance_ledger WHERE state IN ('uploading', 'deletion_pending', 'deleting')")
            .fetch_one(pool).await.map_err(Into::into)
    }
}
