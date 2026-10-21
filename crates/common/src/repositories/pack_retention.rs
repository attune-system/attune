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
        super::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx).await?;
        // Receipt ancestors retain their own admissions even when their original
        // executions are gone. Remove only unreferenced leaves; subsequent
        // collection passes reclaim newly unreferenced ancestors.
        sqlx::query("WITH dormant AS (SELECT receipt.id FROM runtime_input_receipt receipt WHERE NOT runtime_accepted_input_holds_control(receipt.id) AND NOT file_evidence_pinned('runtime_input_receipt',receipt.id) AND NOT EXISTS(SELECT 1 FROM execution e WHERE e.id = receipt.execution) AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_source edge WHERE edge.source_receipt = receipt.id) AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_parent parent WHERE parent.receipt=receipt.id) AND NOT EXISTS(SELECT 1 FROM runtime_native_empty_root_leaf root_leaf WHERE root_leaf.receipt=receipt.id) AND NOT EXISTS(SELECT 1 FROM runtime_secret_origin_reference origin WHERE origin.source_id = receipt.id AND origin.source_entity LIKE 'declared_input:%') ORDER BY receipt.id LIMIT $1) DELETE FROM runtime_input_receipt receipt USING dormant WHERE receipt.id = dormant.id")
            .bind(limit.max(1)).execute(&mut *tx).await?;
        sqlx::query("WITH dormant AS (SELECT input.id FROM runtime_accepted_input input WHERE NOT file_evidence_pinned('accepted_input',input.id) AND NOT EXISTS(SELECT 1 FROM execution e WHERE e.id = input.execution) AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_accepted receipt WHERE receipt.accepted = input.id) ORDER BY input.id LIMIT $1) DELETE FROM runtime_accepted_input input USING dormant WHERE input.id = dormant.id")
            .bind(limit.max(1)).execute(&mut *tx).await?;
        sqlx::query("WITH dormant AS (SELECT input.id FROM runtime_native_input_snapshot input JOIN runtime_authority_admission a ON a.id=input.renderer_admission WHERE NOT file_evidence_pinned('runtime_native_input_snapshot',input.id) AND NOT EXISTS(SELECT 1 FROM execution e WHERE e.id=a.owner_id) AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_snapshot receipt WHERE receipt.snapshot=input.id) AND NOT EXISTS(SELECT 1 FROM runtime_native_cache_frontier frontier WHERE frontier.predecessor=input.id) AND NOT EXISTS(SELECT 1 FROM runtime_native_wait_completion_release completed_wait WHERE completed_wait.original_snapshot=input.id) ORDER BY input.id LIMIT $1) DELETE FROM runtime_native_input_snapshot input USING dormant WHERE input.id=dormant.id")
            .bind(limit.max(1)).execute(&mut *tx).await?;
        super::file_evidence_retention::FileEvidenceRetentionRepository::collect_cache_epochs(
            &mut tx, limit,
        )
        .await?;
        let accepted_items_installed: bool =
            sqlx::query_scalar("SELECT to_regclass('work_queue_item_ad_hoc_dispatch') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await?;
        let accepted_source_pin = if accepted_items_installed {
            "AND NOT EXISTS(SELECT 1 FROM work_queue_item_ad_hoc_dispatch accepted WHERE accepted.source_admission = a.id)"
        } else {
            ""
        };
        // Runtime row retention never blocks on a descendant's authority. The
        // delete trigger leaves referenced admission fences behind; collect only
        // unreferenced leaves here. Ancestors become eligible on the next pass.
        sqlx::query(&format!("WITH dormant AS (SELECT a.id FROM runtime_authority_admission a WHERE NOT file_evidence_pinned('runtime_authority_admission',a.id) AND NOT EXISTS (SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency = a.id) AND NOT EXISTS (SELECT 1 FROM runtime_declared_input_receipt receipt WHERE receipt.source_admission = a.id) AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt WHERE receipt.renderer_admission = a.id OR receipt.destination_admission = a.id) AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_dependency proof WHERE proof.admission = a.id) AND NOT EXISTS(SELECT 1 FROM runtime_accepted_input input WHERE input.admission = a.id) AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot input WHERE input.renderer_admission=a.id) AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_admission input WHERE input.admission=a.id) {accepted_source_pin} AND ((a.owner_kind = 'execution' AND NOT EXISTS (SELECT 1 FROM execution e WHERE e.id = a.owner_id)) OR (a.owner_kind = 'enforcement' AND NOT EXISTS (SELECT 1 FROM enforcement e WHERE e.id = a.owner_id))) ORDER BY a.id FOR UPDATE OF a SKIP LOCKED LIMIT $1) DELETE FROM runtime_authority_admission a USING dormant d WHERE a.id = d.id"))
            .bind(limit.max(1)).execute(&mut *tx).await?;
        // The queue admission component is installed independently during the
        // coordinated rollout. Once present, its FK is an authoritative pin,
        // including terminal items retained for attribution and input evidence.
        let queue_admission_installed: bool = sqlx::query_scalar(
            "SELECT to_regclass('work_queue_item_runtime_admission') IS NOT NULL",
        )
        .fetch_one(&mut *tx)
        .await?;
        let queue_pin = if queue_admission_installed {
            "AND NOT EXISTS (SELECT 1 FROM work_queue_item_runtime_admission queued WHERE queued.policy_revision = p.id)"
        } else {
            ""
        };
        // Retained runtime owners keep historical authority even after completion
        // or revocation. Only dormant revisions outside the rollback window can go.
        sqlx::query(&format!(
            "WITH ranked AS ( \
                 SELECT id, row_number() OVER (PARTITION BY pack ORDER BY inactive_since DESC, id DESC) AS position \
                 FROM pack_release WHERE inactive_since IS NOT NULL \
             ), dormant AS ( \
                 SELECT p.id FROM pack_runtime_policy_revision p \
                 JOIN pack_release r ON r.id = p.release JOIN ranked x ON x.id = r.id \
                 WHERE r.inactive_since < $1 AND x.position > $2 \
                    AND NOT EXISTS (SELECT 1 FROM pack active WHERE active.active_release = r.id) \
                    AND NOT file_evidence_pinned('pack_runtime_policy_revision',p.id) \
                    AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot snapshot WHERE snapshot.policy_revision=p.id) \
                   AND NOT EXISTS (SELECT 1 FROM pack_runtime_policy_current c WHERE c.revision = p.id) \
                     AND NOT EXISTS (SELECT 1 FROM runtime_authority_admission a WHERE a.policy_revision = p.id) \
                      AND NOT EXISTS (SELECT 1 FROM runtime_workflow_policy_dependency graph WHERE graph.dependency = p.id) \
                      AND NOT EXISTS (SELECT 1 FROM runtime_accepted_batch_item accepted WHERE accepted.target_policy = p.id) \
                     AND NOT EXISTS (SELECT 1 FROM runtime_declared_input_binding binding JOIN runtime_declared_input_receipt receipt ON receipt.declaration = binding.id WHERE binding.policy_revision = p.id) \
                    AND NOT EXISTS (SELECT 1 FROM sensor_rule_structural_origin o WHERE o.dispatch_policy_revision = p.id) \
                    {queue_pin} \
                 ORDER BY p.id FOR UPDATE OF p SKIP LOCKED LIMIT $3 \
             ) DELETE FROM pack_runtime_policy_revision p USING dormant d WHERE p.id = d.id",
        ))
        .bind(rollback_cutoff)
        .bind(newest_inactive.max(0))
        .bind(limit.max(1))
        .execute(&mut *tx)
        .await?;
        let candidate_ids: Vec<i64> = sqlx::query_scalar(
            "WITH ranked AS ( \
                 SELECT r.id, row_number() OVER (PARTITION BY r.pack ORDER BY r.inactive_since DESC, r.id DESC) AS position \
                 FROM pack_release r WHERE r.inactive_since IS NOT NULL \
             ) SELECT r.id FROM pack_release r JOIN ranked x ON x.id = r.id \
                  WHERE r.inactive_since < $1 AND x.position > $2 \
                     AND NOT file_evidence_pinned('pack_release',r.id) \
                    AND NOT EXISTS (SELECT 1 FROM pack p WHERE p.active_release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM runtime_authority_release_dependency d WHERE d.release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM managed_sensor_metadata_revision m WHERE m.release = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM sensor_rule_structural_origin o WHERE o.dispatch_source_release = r.id) \
                   AND NOT EXISTS (SELECT 1 FROM execution e WHERE e.pack_release = r.id) \
                   AND NOT EXISTS (SELECT 1 FROM enforcement e WHERE e.pack_release = r.id) \
                   AND NOT EXISTS (SELECT 1 FROM work_queue_item q WHERE q.pack_release = r.id) \
                     AND NOT EXISTS (SELECT 1 FROM sensor_workload s WHERE s.pack_release = r.id) \
                     AND NOT EXISTS (SELECT 1 FROM sensor_generation_schema_binding b WHERE (b.binding->'executable'->'release'->>'id')::BIGINT = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM execution e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM enforcement e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM work_queue_item q CROSS JOIN LATERAL jsonb_each(COALESCE(q.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM runtime_schema_binding b CROSS JOIN LATERAL jsonb_each(COALESCE(b.contract->'static_actions', '{}'::jsonb)) child WHERE (child.value->'executable_snapshot'->'release'->>'id')::BIGINT = r.id) \
                    AND NOT EXISTS (SELECT 1 FROM runtime_schema_binding b CROSS JOIN LATERAL jsonb_each(COALESCE(b.contract->'static_actions', '{}'::jsonb)) child CROSS JOIN LATERAL jsonb_each(COALESCE(child.value->'executable_snapshot'->'pack_executables', '{}'::jsonb)) nested WHERE (nested.value->'release'->>'id')::BIGINT = r.id) \
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
               AND NOT EXISTS (SELECT 1 FROM runtime_authority_release_dependency d WHERE d.release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM managed_sensor_metadata_revision m WHERE m.release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM sensor_rule_structural_origin o WHERE o.dispatch_source_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM execution e WHERE e.pack_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM enforcement e WHERE e.pack_release = r.id) \
               AND NOT EXISTS (SELECT 1 FROM work_queue_item q WHERE q.pack_release = r.id) \
                AND NOT EXISTS (SELECT 1 FROM sensor_workload s WHERE s.pack_release = r.id) \
                AND NOT EXISTS (SELECT 1 FROM sensor_generation_schema_binding b WHERE (b.binding->'executable'->'release'->>'id')::BIGINT = r.id) \
               AND NOT EXISTS (SELECT 1 FROM execution e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
               AND NOT EXISTS (SELECT 1 FROM enforcement e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
               AND NOT EXISTS (SELECT 1 FROM work_queue_item q CROSS JOIN LATERAL jsonb_each(COALESCE(q.executable_snapshot->'pack_executables', '{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT = r.id) \
               AND NOT EXISTS (SELECT 1 FROM runtime_schema_binding b CROSS JOIN LATERAL jsonb_each(COALESCE(b.contract->'static_actions', '{}'::jsonb)) child WHERE (child.value->'executable_snapshot'->'release'->>'id')::BIGINT = r.id) \
               AND NOT EXISTS (SELECT 1 FROM runtime_schema_binding b CROSS JOIN LATERAL jsonb_each(COALESCE(b.contract->'static_actions', '{}'::jsonb)) child CROSS JOIN LATERAL jsonb_each(COALESCE(child.value->'executable_snapshot'->'pack_executables', '{}'::jsonb)) nested WHERE (nested.value->'release'->>'id')::BIGINT = r.id) \
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
               AND NOT file_evidence_pinned('pack_release',r.id) \
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
