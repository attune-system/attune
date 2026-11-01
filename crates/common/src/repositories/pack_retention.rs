use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use std::path::Path;
use std::time::Duration;

use crate::pack_registry::PackStorage;
use crate::{Error, Result};

mod bounded;
mod queries;

pub use bounded::{
    GraphReclamationOptions, GraphReclamationOutcome, GraphReclamationPhase,
    GraphReclamationPhaseResult, GraphReclamationProgress, GraphReclamationResult,
};

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
    /// Earliest durable graph deadline. This cadence is independent of row
    /// retention; a pending pass must not wait for the next hourly retention job.
    pub async fn next_reclamation_attempt(pool: &PgPool) -> Result<Option<DateTime<Utc>>> {
        // Initial rows use -infinity. Normalize only at the API boundary since
        // chrono cannot represent PostgreSQL's infinite timestamp sentinel.
        let phases = GraphReclamationPhase::ALL.map(GraphReclamationPhase::name);
        Ok(sqlx::query_scalar(
            "SELECT CASE WHEN MIN(next_attempt)='-infinity'::timestamptz THEN clock_timestamp() ELSE MIN(next_attempt) END FROM graph_reclamation_progress WHERE phase=ANY($1::text[])",
        )
        .bind(&phases[..])
        .fetch_one(pool)
        .await?)
    }

    pub async fn reclamation_due(pool: &PgPool) -> Result<bool> {
        let phases = GraphReclamationPhase::ALL.map(GraphReclamationPhase::name);
        Ok(sqlx::query_scalar(
            "SELECT COALESCE(MIN(next_attempt)<=clock_timestamp(),false) FROM graph_reclamation_progress WHERE phase=ANY($1::text[])",
        )
        .bind(&phases[..])
        .fetch_one(pool)
        .await?)
    }

    /// Bounded-size status read without graph admission. Never interpret retained
    /// progress or a successful outer maintenance cycle as current authority.
    pub async fn reclamation_progress(pool: &PgPool) -> Result<Vec<GraphReclamationProgress>> {
        let phases = GraphReclamationPhase::ALL.map(GraphReclamationPhase::name);
        Ok(sqlx::query_as(
            "SELECT phase,cursor_id AS cursor,high_water,probe_limit,pass_deferred,oversized_since,
                NULLIF(next_attempt,'-infinity'::timestamptz) AS next_attempt,
                last_attempt,last_progress,LEAST(deferred_since,oversized_since) AS deferred_since,
                cursor_id<>0 OR high_water<>0 OR pass_deferred OR deferred_since IS NOT NULL OR oversized_since IS NOT NULL AS pending
             FROM graph_reclamation_progress WHERE phase=ANY($1::text[]) ORDER BY phase",
        )
        .bind(&phases[..])
        .fetch_all(pool)
        .await?)
    }

    /// Metadata only. Each leaf phase commits independently. Callers must inspect
    /// every phase outcome, including deferrals, and keep one scheduled collector
    /// active under the existing supervisor leader. No filesystem I/O occurs here.
    pub async fn collect_bounded(
        pool: &PgPool,
        rollback_cutoff: DateTime<Utc>,
        newest_inactive: i64,
        options: &GraphReclamationOptions,
    ) -> Result<GraphReclamationResult> {
        bounded::collect(
            pool,
            rollback_cutoff,
            newest_inactive.max(0),
            options,
            bounded::CollectionMode::Scheduled,
        )
        .await
    }

    /// Compatibility collection performs one forced page per phase and waits
    /// briefly for admission. Success describes those pages, not a drained scan.
    /// Scheduled callers must use collect_bounded and its durable deadlines.
    pub async fn collect(
        pool: &PgPool,
        packs_base_dir: &Path,
        rollback_cutoff: DateTime<Utc>,
        newest_inactive: i64,
        limit: i64,
    ) -> Result<PackRetentionResult> {
        let options = GraphReclamationOptions {
            delete_limit: limit.clamp(1, 256),
            statement_timeout: Duration::from_secs(2),
            page_timeout: Duration::from_secs(5),
            cycle_timeout: Duration::from_secs(40),
            queued_lock_timeout: Duration::from_secs(1),
            ..GraphReclamationOptions::default()
        };
        // Existing direct callers wait for short in-flight publications, rather
        // than mistaking a try-lock deferral for a successful collection. New
        // scheduled maintenance uses collect_bounded's persisted retry policy.
        let result = bounded::collect(
            pool,
            rollback_cutoff,
            newest_inactive.max(0),
            &options,
            bounded::CollectionMode::Compatibility,
        )
        .await?;
        // Physical deletion retains its separate publication/table-lock fence.
        // These identities belong only to confirmed metadata commits.
        Self::cleanup_unreferenced(pool, packs_base_dir, &result.deleted_content).await?;
        if !result.pages_succeeded() {
            return Err(Error::invalid_state(
                "Graph reclamation failed or deferred; inspect collect_bounded phase outcomes",
            ));
        }
        // Exact reporting count observed AFTER all metadata transactions, never
        // under exclusive graph admission. Concurrent publication may change it.
        let retained = Self::retained_count(pool).await?;
        Ok(PackRetentionResult {
            retained,
            deleted: result.deleted_content.len() as u64,
        })
    }

    /// Exact, separately observed reporting count. Do not call with graph
    /// admission held or use it as a candidate/backlog discovery query.
    pub async fn retained_count(pool: &PgPool) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM pack_release")
            .fetch_one(pool)
            .await?)
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
