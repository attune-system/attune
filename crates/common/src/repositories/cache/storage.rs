//! Storage observations and planner statistics, separate from reclamation.

use crate::{config::CacheRetentionConfig, Error, Result};
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct CacheStorageObservation {
    pub registered_partitions: i64,
    pub partitions_created: i64,
    pub partitions_dropped: i64,
    pub cleanup_backlog: i64,
    pub oldest_cleanup_age_seconds: i64,
    pub statistics_pending: bool,
    pub last_analyzed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatisticsRefreshOutcome {
    Applied,
    NotDue,
    DeferredBusy,
    DeferredDeadline,
}

pub struct CacheStorageRepository;

impl CacheStorageRepository {
    /// Counts metadata only, without scanning entry rows.
    pub async fn observe(
        pool: &PgPool,
        config: &CacheRetentionConfig,
    ) -> Result<CacheStorageObservation> {
        config
            .validate_storage_maintenance()
            .map_err(Error::validation)?;
        let traversal_seconds = i64::try_from(config.min_traversal_window_seconds)
            .map_err(|_| Error::validation("cache traversal window exceeds BIGINT"))?;
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true)")
            .bind(format!("{}ms", config.ddl_lock_timeout_milliseconds))
            .bind(format!("{}ms", config.statistics_statement_timeout_milliseconds))
            .execute(&mut *tx).await?;
        let observation = sqlx::query_as::<_, CacheStorageObservation>(
            "WITH eligible AS (
                SELECT g.id, CASE WHEN g.state = 'failed' THEN COALESCE(g.failed, g.created)
                       ELSE GREATEST(g.readable_until,
                            g.retired + make_interval(secs => $1::DOUBLE PRECISION)) END AS eligible_since
                FROM cache_generation g JOIN cache_namespace n ON n.id = g.namespace
                WHERE n.active_generation IS DISTINCT FROM g.id
                  AND (g.state = 'failed' OR (g.state = 'retired'
                       AND g.readable_until <= clock_timestamp()
                       AND g.retired <= clock_timestamp() - make_interval(secs => $1::DOUBLE PRECISION)))
                  AND NOT EXISTS (SELECT 1 FROM workflow_cache_iteration i
                       JOIN workflow_execution w ON w.id = i.workflow_execution
                       WHERE i.generation = g.id AND i.state = 'scanning'
                         AND w.status NOT IN ('completed','failed','cancelled','timeout','abandoned'))
             ) SELECT
                (SELECT COUNT(*) FROM pg_inherits WHERE inhparent = 'cache_entry'::REGCLASS) AS registered_partitions,
                s.partitions_created, s.partitions_dropped,
                (SELECT COUNT(*) FROM eligible) AS cleanup_backlog,
                COALESCE((SELECT GREATEST(0, EXTRACT(EPOCH FROM clock_timestamp() - MIN(eligible_since)))::BIGINT
                          FROM eligible), 0)::BIGINT AS oldest_cleanup_age_seconds,
                s.requested_revision > s.completed_revision AS statistics_pending, s.last_analyzed_at
             FROM cache_entry_statistics_state s WHERE id = TRUE",
        )
        .bind(traversal_seconds)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(observation)
    }

    /// PG16+ ANALYZE samples the parent and leaves. A server deadline bounds
    /// sampling across the partition cap. A concurrent lifecycle revision is
    /// not acknowledged by this snapshot and remains pending for another pass.
    pub async fn refresh_statistics(
        pool: &PgPool,
        config: &CacheRetentionConfig,
    ) -> Result<CacheStatisticsRefreshOutcome> {
        config
            .validate_storage_maintenance()
            .map_err(Error::validation)?;
        if config.dry_run {
            return Ok(CacheStatisticsRefreshOutcome::NotDue);
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true)")
            .bind(format!("{}ms", config.ddl_lock_timeout_milliseconds))
            .bind(format!("{}ms", config.statistics_statement_timeout_milliseconds))
            .execute(&mut *tx).await?;
        let result: Result<bool> = async {
            sqlx::query("LOCK TABLE ONLY cache_entry IN SHARE UPDATE EXCLUSIVE MODE")
                .execute(&mut *tx).await?;
            let owns_parent: bool = sqlx::query_scalar(
                "SELECT pg_has_role(current_user, relowner, 'USAGE') FROM pg_class WHERE oid='cache_entry'::REGCLASS AND relkind='p'",
            ).fetch_one(&mut *tx).await?;
            if !owns_parent {
                return Err(Error::invalid_state("cache statistics maintenance requires effective parent ownership"));
            }
            let revision: Option<i64> = sqlx::query_scalar(
                "SELECT requested_revision FROM cache_entry_statistics_state WHERE id=TRUE
                 AND requested_revision > completed_revision
                 AND (last_analyzed_at IS NULL OR last_analyzed_at <= clock_timestamp() - make_interval(secs => $1::DOUBLE PRECISION))",
            ).bind(config.statistics_interval_seconds as i64)
                .fetch_optional(&mut *tx).await?;
            let Some(revision) = revision else { return Ok(false); };
            // Supported reads don't filter JSON fields. Avoid TOAST sampling.
            sqlx::query("ANALYZE cache_entry (generation, external_id, id, size_bytes)")
                .execute(&mut *tx).await?;
            let updated = sqlx::query(
                "UPDATE cache_entry_statistics_state SET completed_revision=GREATEST(completed_revision,$1),
                 last_analyzed_at=clock_timestamp() WHERE id=TRUE",
            ).bind(revision).execute(&mut *tx).await?;
            if updated.rows_affected() != 1 {
                return Err(Error::invalid_state("cache statistics state disappeared"));
            }
            Ok(true)
        }.await;
        match result {
            Ok(applied) => {
                tx.commit().await?;
                Ok(if applied {
                    CacheStatisticsRefreshOutcome::Applied
                } else {
                    CacheStatisticsRefreshOutcome::NotDue
                })
            }
            Err(error) => {
                let code = match &error {
                    Error::Database(error) => error
                        .as_database_error()
                        .and_then(|e| e.code())
                        .map(|c| c.into_owned()),
                    _ => None,
                };
                tx.rollback().await?;
                match code.as_deref() {
                    Some("55P03") => Ok(CacheStatisticsRefreshOutcome::DeferredBusy),
                    Some("57014") => Ok(CacheStatisticsRefreshOutcome::DeferredDeadline),
                    _ => Err(error),
                }
            }
        }
    }
}
