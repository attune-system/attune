use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use std::path::Path;

use crate::pack_registry::PackStorage;
use crate::Result;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PackRetentionResult {
    pub retained: i64,
    pub deleted: u64,
}

pub struct PackRetentionRepository;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PackReleaseContent {
    pub digest: String,
    pub content_path: String,
}

impl PackRetentionRepository {
    pub async fn collect(
        pool: &PgPool,
        packs_base_dir: &Path,
        rollback_cutoff: DateTime<Utc>,
        newest_inactive: i64,
        limit: i64,
    ) -> Result<PackRetentionResult> {
        let mut tx = pool.begin().await?;
        let candidate_ids: Vec<i64> = sqlx::query_scalar(
            "WITH ranked AS ( \
                 SELECT r.id, row_number() OVER (PARTITION BY r.pack ORDER BY r.inactive_since DESC, r.id DESC) AS position \
                 FROM pack_release r WHERE r.inactive_since IS NOT NULL \
             ) SELECT r.id FROM pack_release r JOIN ranked x ON x.id = r.id \
                 WHERE r.inactive_since < $1 AND x.position > $2 \
                   AND NOT EXISTS (SELECT 1 FROM pack p WHERE p.active_release = r.id) \
                   AND NOT EXISTS (SELECT 1 FROM execution e WHERE e.pack_release = r.id) \
                   AND NOT EXISTS (SELECT 1 FROM enforcement e WHERE e.pack_release = r.id) \
                   AND NOT EXISTS (SELECT 1 FROM work_queue_item q WHERE q.pack_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM sensor_workload s WHERE s.pack_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM execution e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM enforcement e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM work_queue_item q CROSS JOIN LATERAL jsonb_each(COALESCE(q.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM runtime c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM runtime_version c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM permission_set c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM trigger c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM action c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM sensor c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM rule c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM policy c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM work_queue c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM workflow_definition c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM dashboard c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM cache_namespace c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
                 ORDER BY r.inactive_since, r.id FOR UPDATE OF r SKIP LOCKED LIMIT $3",
        )
        .bind(rollback_cutoff)
        .bind(newest_inactive.max(0))
        .bind(limit.max(1))
        .fetch_all(&mut *tx)
        .await?;
        let deleted_content: Vec<PackReleaseContent> = sqlx::query_as(
            "DELETE FROM pack_release r WHERE r.id = ANY($1) \
               AND NOT EXISTS (SELECT 1 FROM pack p WHERE p.active_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM execution e WHERE e.pack_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM enforcement e WHERE e.pack_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM work_queue_item q WHERE q.pack_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM sensor_workload s WHERE s.pack_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM execution e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
               AND NOT EXISTS (SELECT 1 FROM enforcement e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
               AND NOT EXISTS (SELECT 1 FROM work_queue_item q CROSS JOIN LATERAL jsonb_each(COALESCE(q.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
               AND NOT EXISTS (SELECT 1 FROM runtime c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM runtime_version c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM permission_set c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM trigger c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM action c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM sensor c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM rule c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM policy c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM work_queue c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM workflow_definition c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM dashboard c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM cache_namespace c WHERE c.retired_at IS NULL AND c.managed_release = r.id) \
              RETURNING r.digest, r.content_path",
        )
        .bind(&candidate_ids)
        .fetch_all(&mut *tx)
        .await?;
        let retained = sqlx::query_scalar("SELECT COUNT(*) FROM pack_release")
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Self::cleanup_unreferenced(pool, packs_base_dir, &deleted_content).await?;
        Ok(PackRetentionResult {
            retained,
            deleted: deleted_content.len() as u64,
        })
    }

    pub async fn content_for_pack(
        transaction: &mut Transaction<'_, Postgres>,
        pack: i64,
    ) -> Result<Vec<PackReleaseContent>> {
        Ok(sqlx::query_as(
            "SELECT digest, content_path FROM pack_release WHERE pack = $1 ORDER BY id",
        )
        .bind(pack)
        .fetch_all(&mut **transaction)
        .await?)
    }

    /// Call only after the transaction deleting these release rows commits.
    pub async fn cleanup_unreferenced(
        pool: &PgPool,
        packs_base_dir: &Path,
        deleted_content: &[PackReleaseContent],
    ) -> Result<u64> {
        if deleted_content.is_empty() {
            return Ok(0);
        }

        let storage = PackStorage::new(packs_base_dir);
        let mut tx = pool.begin().await?;
        sqlx::query("LOCK TABLE pack_release IN SHARE MODE")
            .execute(&mut *tx)
            .await?;
        let mut removed = 0;
        for content in deleted_content {
            let referenced: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pack_release WHERE digest = $1 OR content_path = $2)",
            )
            .bind(&content.digest)
            .bind(&content.content_path)
            .fetch_one(&mut *tx)
            .await?;
            if !referenced {
                match storage.remove_release_tree(&content.digest, &content.content_path) {
                    Ok(true) => removed += 1,
                    Ok(false) => {}
                    Err(error) => tracing::warn!(
                        error = %error,
                        digest = %content.digest,
                        "Skipped unsafe or unreadable local pack release tree"
                    ),
                }
            }
        }
        tx.commit().await?;
        Ok(removed)
    }
}
