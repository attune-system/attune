use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgConnection, PgPool, Postgres, Transaction};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::maintenance_admission::{self, MaintenanceBudget, MaintenanceResult};
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
    async fn cleanup_transaction(
        pool: &PgPool,
        id: i64,
    ) -> Result<Option<Transaction<'static, Postgres>>> {
        let mut tx = pool.begin().await?;
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx)
            .await?;
        if super::file_evidence_retention::FileEvidenceRetentionRepository::request_cleanup(
            &mut tx,
            &[id],
        )
        .await?
            != [id]
        {
            tx.rollback().await?;
            return Ok(None);
        }
        Ok(Some(tx))
    }
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
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx)
            .await?;
        let claimed = Self::claim_pending_with_conn(&mut tx, id, cutoff, shared_file, None).await?;
        tx.commit().await?;
        Ok(claimed)
    }

    async fn claim_pending_with_conn(
        conn: &mut PgConnection,
        id: i64,
        cutoff: DateTime<Utc>,
        shared_file: bool,
        observed: Option<&str>,
    ) -> Result<bool> {
        if super::file_evidence_retention::FileEvidenceRetentionRepository::request_cleanup(
            conn,
            &[id],
        )
        .await?
            != [id]
        {
            return Ok(false);
        }
        let current: Option<i64> = sqlx::query_scalar("SELECT id FROM artifact_version WHERE id = $1 AND ($2::text IS NULL OR xmin::text=$2) FOR UPDATE")
            .bind(id)
            .bind(observed)
            .fetch_optional(&mut *conn)
            .await?;
        if current.is_none() {
            return Ok(false);
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('log_stream'), hashtext($1::text))")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("SELECT id FROM log_stream WHERE artifact_version = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
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
            .execute(&mut *conn)
            .await?
            .rows_affected()
            == 1;
        let already_claimed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM artifact_version WHERE id = $1 AND body_state = 'cleanup_claimed')",
        )
        .bind(id)
        .fetch_one(conn)
        .await?;
        Ok(claimed || already_claimed)
    }

    pub async fn claim_abandoned_pending_bounded(
        pool: &PgPool,
        id: i64,
        cutoff: DateTime<Utc>,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<bool> {
        maintenance_admission::run(pool, budget, deadline, cancel,
            move |conn| Box::pin(async move {
                Ok(sqlx::query_scalar::<_, String>("SELECT xmin::text FROM artifact_version WHERE id=$1 AND object_key IS NOT NULL AND (body_state='cleanup_claimed' OR (body_state='pending' AND body_updated<$2))")
                    .bind(id).bind(cutoff).fetch_all(conn).await?)
            }),
            move |conn, candidates| Box::pin(async move {
                // Preserve graph -> owner -> artifact -> stream lock order.
                // A stale/ineligible candidate must not commit an owner claim.
                use sqlx::Acquire;
                let mut claim = conn.begin().await?;
                let claimed = Self::claim_pending_with_conn(&mut claim,id,cutoff,false,Some(&candidates[0])).await?;
                if claimed { claim.commit().await?; } else { claim.rollback().await?; }
                Ok((claimed,u64::from(claimed)))
            }),
        ).await
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
             FROM artifact_version av WHERE {predicate} AND file_evidence_version_eligible(av.id) ORDER BY av.body_updated, av.id LIMIT $2"
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
        let Some(mut tx) = Self::cleanup_transaction(pool, id).await? else {
            return Ok(false);
        };
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
        .execute(&mut *tx)
        .await?;
        let changed = result.rows_affected() == 1;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn mark_log_deleting_from_pending(
        pool: &PgPool,
        id: i64,
        segment_count: i64,
        size_bytes: i64,
    ) -> Result<bool> {
        let Some(mut tx) = Self::cleanup_transaction(pool, id).await? else {
            return Ok(false);
        };
        let result = sqlx::query(
            "UPDATE artifact_version SET body_state = 'deleting', provider_version = $2, \
             size_bytes = $3, sha256 = repeat('0', 64) \
             WHERE id = $1 AND body_state = 'pending'",
        )
        .bind(id)
        .bind(format!("segments:{segment_count}"))
        .bind(size_bytes)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_deleting(pool: &PgPool, id: i64) -> Result<bool> {
        let Some(mut tx) = Self::cleanup_transaction(pool, id).await? else {
            return Ok(false);
        };
        let result = sqlx::query(
            "UPDATE artifact_version SET body_state = 'deleting' \
             WHERE id = $1 AND body_state = 'ready'",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn delete_cleanup_claimed(pool: &PgPool, id: i64) -> Result<bool> {
        let mut conn = pool.acquire().await?;
        Self::delete_cleanup_claimed_with_conn(&mut conn, id, None).await
    }

    async fn delete_cleanup_claimed_with_conn(
        conn: &mut PgConnection,
        id: i64,
        observed: Option<&str>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM artifact_version av \
             WHERE av.id = $1 AND ($2::text IS NULL OR av.xmin::text=$2) AND av.body_state = 'cleanup_claimed' \
             AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g \
                             WHERE g.artifact_version = av.id AND g.state = 'issued' \
                               AND g.settle_until > clock_timestamp())",
        )
        .bind(id)
        .bind(observed)
        .execute(conn)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn delete_cleanup_claimed_bounded(
        pool: &PgPool,
        id: i64,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<bool> {
        maintenance_admission::run(pool, budget, deadline, cancel,
            move |conn| Box::pin(async move {
                Ok(sqlx::query_scalar::<_, String>("SELECT xmin::text FROM artifact_version WHERE id=$1 AND body_state='cleanup_claimed'")
                    .bind(id).fetch_all(conn).await?)
            }),
            move |conn, candidates| Box::pin(async move {
                // DELETE triggers recheck pins and preserve physical obligations
                // in the same transaction, already holding exclusive admission.
                let deleted = Self::delete_cleanup_claimed_with_conn(conn,id,Some(&candidates[0])).await?;
                Ok((deleted,u64::from(deleted)))
            }),
        ).await
    }

    /// The version is the durable retry candidate for both operations. Commit
    /// its deletion only together with parent metadata refresh/deletion.
    pub async fn delete_cleanup_claimed_and_refresh_bounded(
        pool: &PgPool,
        id: i64,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<bool> {
        maintenance_admission::run(pool,budget,deadline,cancel,
            move |conn| Box::pin(async move {
                Ok(sqlx::query_as::<_,(i64,String,String)>("SELECT av.artifact,av.xmin::text,a.xmin::text FROM artifact_version av JOIN artifact a ON a.id=av.artifact WHERE av.id=$1 AND av.body_state='cleanup_claimed'")
                    .bind(id).fetch_all(conn).await?)
            }),
            move |conn,candidates| Box::pin(async move {
                use sqlx::Acquire;
                let (artifact,version_xmin,artifact_xmin) = &candidates[0];
                let mut unit = conn.begin().await?;
                let deleted = async {
                    // Graph admission is already exclusive. Protect the whole
                    // evidence cleanup group before targets; release triggers
                    // may touch peer owners. Preserve the existing group/pins.
                    sqlx::query_scalar::<_,i64>("SELECT version FROM file_evidence_owner WHERE version=$1 OR cleanup_group=(SELECT cleanup_group FROM file_evidence_owner WHERE version=$1) ORDER BY version FOR UPDATE")
                        .bind(id).fetch_all(&mut *unit).await?;
                    // Match version writers' advisory -> artifact-row order,
                    // and acquire the artifact before locking its version.
                    if super::artifact::ArtifactRepository::find_by_id_for_update(&mut unit,*artifact).await?.is_none() {
                        return Ok::<_,crate::Error>(false);
                    }
                    let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact WHERE id=$1 AND xmin::text=$2)")
                        .bind(artifact).bind(artifact_xmin).fetch_one(&mut *unit).await?;
                    if !current { return Ok(false); }
                    let current: Option<i64> = sqlx::query_scalar("SELECT id FROM artifact_version av WHERE id=$1 AND artifact=$2 AND xmin::text=$3 AND body_state='cleanup_claimed' AND file_evidence_version_eligible(id) AND NOT EXISTS(SELECT 1 FROM artifact_upload_grant g WHERE g.artifact_version=av.id AND g.state='issued' AND g.settle_until>clock_timestamp()) FOR UPDATE")
                        .bind(id).bind(artifact).bind(version_xmin).fetch_optional(&mut *unit).await?;
                    if current.is_none() { return Ok(false); }
                    if !Self::delete_cleanup_claimed_with_conn(&mut unit,id,Some(version_xmin)).await? {
                        return Ok(false);
                    }
                    // This helper checks ALL remaining versions before deleting
                    // the artifact, not merely the ready-version frontier.
                    super::maintenance::MaintenanceRepository::refresh_or_delete_artifact_metadata_with_conn(&mut unit,*artifact).await?;
                    Ok(true)
                }.await?;
                // BEFORE DELETE triggers may write even when a later trigger
                // suppresses deletion. A no-op must publish none of that work.
                if deleted { unit.commit().await?; } else { unit.rollback().await?; }
                Ok((deleted,u64::from(deleted)))
            }),
        ).await
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
              AND legacy_snapshot_expires_at <= NOW() AND NOT file_evidence_pinned('pack_release',id) ORDER BY id LIMIT $1",
        )
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn clear_pack_snapshot<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let result = sqlx::query(
            "UPDATE pack_release SET archive_path = NULL, legacy_snapshot_expires_at = NULL \
             WHERE id = $1 AND legacy_snapshot_expires_at <= NOW()",
        )
        .bind(id)
        .execute(executor)
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

    pub async fn clear_artifact_snapshot<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let result = sqlx::query(
            "UPDATE artifact_version SET file_path = NULL, legacy_snapshot_expires_at = NULL \
             WHERE id = $1 AND legacy_snapshot_expires_at <= NOW()",
        )
        .bind(id)
        .execute(executor)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}
