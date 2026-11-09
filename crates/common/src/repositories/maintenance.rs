//! Supervisor maintenance repository helpers.

use chrono::{DateTime, Duration, Utc};
use sqlx::{FromRow, PgConnection, PgPool, Row};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::maintenance_admission::{self, MaintenanceBudget, MaintenanceResult};
use crate::{
    models::{Id, JsonDict},
    Result,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorRunStart {
    pub run_id: String,
    pub dirty_shutdown_detected: bool,
}

#[derive(Debug, Clone, FromRow)]
pub struct ExpiredArtifactVersion {
    pub id: i64,
    pub artifact: i64,
    pub version: i32,
    pub file_path: Option<String>,
    pub body_state: Option<crate::models::enums::ArtifactBodyState>,
    pub object_key: Option<String>,
    pub provider_version: Option<String>,
}

/// One raw oldest-first discovery page. These candidates are advisory only.
#[derive(Debug)]
pub struct ArtifactDiscoveryPage {
    pub versions: Vec<ExpiredArtifactVersion>,
    pub scanned: usize,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactCleanupResult {
    /// Versions considered in this page, not an exact count of the backlog.
    pub candidates: i64,
    pub scan_pending: bool,
    pub deleted_versions: i64,
    pub deleted_files: i64,
    pub deleted_artifacts: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StuckRuntimeSnapshot {
    pub kind: &'static str,
    pub status: String,
    pub count: i64,
    pub oldest: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct StaleExecutionCandidate {
    pub id: Id,
    pub action: Option<Id>,
    pub action_ref: String,
    pub status: String,
    pub updated: DateTime<Utc>,
    pub worker: Option<Id>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ExecutionRescheduleCandidate {
    pub execution_id: Id,
    pub action_id: Option<Id>,
    pub action_ref: String,
    pub parent_id: Option<Id>,
    pub enforcement_id: Option<Id>,
    pub config: Option<JsonDict>,
    pub release_id: Option<Id>,
    pub release_digest: Option<String>,
    pub attempt_count: i32,
    pub last_attempt_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ExecutionRescheduleAttempt {
    pub execution_id: Id,
    pub action_id: Option<Id>,
    pub action_ref: String,
    pub parent_id: Option<Id>,
    pub enforcement_id: Option<Id>,
    pub config: Option<JsonDict>,
    pub release_id: Option<Id>,
    pub release_digest: Option<String>,
    pub attempt_count: i32,
    pub last_attempt_at: DateTime<Utc>,
    pub last_reason: Option<String>,
    pub last_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRemediationResult {
    pub dispatches_corrected: i64,
    pub items_corrected: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRemediationResult {
    pub entries_removed: i64,
    pub active_entries_removed: i64,
    pub promoted_execution_ids: Vec<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRemediationResult {
    pub workflow_executions_corrected: i64,
    pub parent_executions_corrected: Vec<Id>,
}

pub struct MaintenanceRepository;

impl MaintenanceRepository {
    pub async fn start_supervisor_run(
        pool: &PgPool,
        service_name: &str,
        instance_id: &str,
        run_id: &str,
    ) -> Result<SupervisorRunStart> {
        let mut tx = pool.begin().await?;
        let dirty_shutdown_detected = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1
                FROM supervisor_run
                WHERE service_name = $1
                  AND clean_shutdown = FALSE
                  AND stopped_at IS NULL
            )",
        )
        .bind(service_name)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE supervisor_run
             SET stopped_at = NOW(),
                 stop_reason = 'dirty_shutdown_detected_by_supervisor'
             WHERE service_name = $1
               AND clean_shutdown = FALSE
               AND stopped_at IS NULL",
        )
        .bind(service_name)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO supervisor_run (
                id, service_name, instance_id, started_at, heartbeat_at, clean_shutdown, meta
             )
             VALUES ($1, $2, $3, NOW(), NOW(), FALSE, '{}'::jsonb)",
        )
        .bind(run_id)
        .bind(service_name)
        .bind(instance_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(SupervisorRunStart {
            run_id: run_id.to_string(),
            dirty_shutdown_detected,
        })
    }

    pub async fn heartbeat_supervisor_run(pool: &PgPool, run_id: &str) -> Result<()> {
        sqlx::query("UPDATE supervisor_run SET heartbeat_at = NOW() WHERE id = $1")
            .bind(run_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn mark_supervisor_run_clean(
        pool: &PgPool,
        run_id: &str,
        stop_reason: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE supervisor_run
             SET heartbeat_at = NOW(),
                 stopped_at = NOW(),
                 clean_shutdown = TRUE,
                 stop_reason = $2
             WHERE id = $1",
        )
        .bind(run_id)
        .bind(stop_reason)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn expired_artifact_version_count(pool: &PgPool) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(&format!(
            "{} SELECT COUNT(*)::BIGINT FROM expired av WHERE file_evidence_version_eligible(av.id)",
            expired_artifact_prefilter()
        ))
        .fetch_one(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn find_expired_artifact_versions(
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<ExpiredArtifactVersion>> {
        sqlx::query_as::<_, ExpiredArtifactVersion>(&format!(
            "{} SELECT av.id, av.artifact, av.version, av.file_path, av.body_state, av.object_key, av.provider_version
             FROM expired av
             WHERE file_evidence_version_eligible(av.id)
             ORDER BY av.created ASC, av.id ASC
             LIMIT $1",
            expired_artifact_prefilter()
        ))
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    /// Scan at most `limit` raw rows (hard capped at 256), before retention or
    /// evidence evaluation. No separate output cap can discard eligible rows.
    /// Each pass freezes its greatest (created,id); a completed pass wraps on
    /// the next invocation, revisiting pins, policy changes and late commits.
    pub async fn expired_artifact_candidates_bounded(
        pool: &PgPool,
        limit: i64,
        budget: &MaintenanceBudget,
    ) -> Result<ArtifactDiscoveryPage> {
        maintenance_admission::discovery(pool, budget, move |conn| Box::pin(async move {
            type Cursor = (DateTime<Utc>, i64);
            let (mut last_created, mut last_id, mut high_created, mut high_id):
                (Option<DateTime<Utc>>, Option<i64>, Option<DateTime<Utc>>, Option<i64>) =
                sqlx::query_as("SELECT last_created,last_id,high_created,high_id FROM artifact_retention_discovery WHERE singleton FOR UPDATE")
                    .fetch_one(&mut *conn).await?;
            if high_id.is_none() || (last_created,last_id) == (high_created,high_id) {
                let high: Option<Cursor> = sqlx::query_as("SELECT created,id FROM artifact_version ORDER BY created DESC,id DESC LIMIT 1")
                    .fetch_optional(&mut *conn).await?;
                last_created = None;
                last_id = None;
                high_created = high.map(|v| v.0);
                high_id = high.map(|v| v.1);
            }
            // The index-supported raw page is materialized before either expensive
            // predicate, including when every row is unexpired or pinned.
            let raw: Vec<Cursor> = sqlx::query_as("SELECT created,id FROM artifact_version
                WHERE (created,id)>(COALESCE($1::timestamptz,'-infinity'::timestamptz),COALESCE($2::bigint,'-9223372036854775808'::bigint))
                  AND (created,id)<=($3,$4) ORDER BY created,id LIMIT $5")
                .bind(last_created).bind(last_id).bind(high_created).bind(high_id)
                .bind(limit.clamp(1,256)).fetch_all(&mut *conn).await?;
            let ids: Vec<i64> = raw.iter().map(|v| v.1).collect();
            let versions = sqlx::query_as(&format!("WITH raw AS MATERIALIZED (
                    SELECT id,artifact,version,file_path,body_state,object_key,provider_version,created
                    FROM artifact_version WHERE id=ANY($1)),
                expired AS MATERIALIZED (SELECT av.* FROM raw av JOIN artifact a ON a.id=av.artifact WHERE {})
                SELECT av.id,av.artifact,av.version,av.file_path,av.body_state,av.object_key,av.provider_version
                FROM expired av WHERE file_evidence_version_eligible(av.id) ORDER BY av.created,av.id",
                expired_artifact_version_predicate()))
                .bind(&ids).fetch_all(&mut *conn).await?;
            let next = raw.last().copied();
            let (next_created,next_id) = next.map(|v| (Some(v.0),Some(v.1))).unwrap_or((high_created,high_id));
            sqlx::query("UPDATE artifact_retention_discovery SET last_created=$1,last_id=$2,high_created=$3,high_id=$4 WHERE singleton")
                .bind(next_created).bind(next_id).bind(high_created).bind(high_id).execute(conn).await?;
            Ok(ArtifactDiscoveryPage { versions, scanned: raw.len(), pending: (next_created,next_id)!=(high_created,high_id) })
        })).await
    }

    /// The caller retains physical admission until both deletion and parent
    /// refresh commit. A suppressed deletion must roll back incidental triggers.
    pub async fn delete_artifact_version_and_refresh_with_conn(
        conn: &mut PgConnection,
        id: i64,
        artifact: i64,
    ) -> Result<(bool, bool)> {
        let deleted = sqlx::query("DELETE FROM artifact_version WHERE id=$1 AND artifact=$2")
            .bind(id)
            .bind(artifact)
            .execute(&mut *conn)
            .await?
            .rows_affected()
            == 1;
        if !deleted {
            return Ok((false, false));
        }
        let (parent_deleted, _) =
            Self::refresh_or_delete_artifact_metadata_with_conn(conn, artifact).await?;
        Ok((true, parent_deleted))
    }

    pub async fn orphaned_staging_bounded(
        pool: &PgPool,
        limit: i64,
        budget: &MaintenanceBudget,
    ) -> Result<Vec<(String, i64)>> {
        maintenance_admission::probe(pool,budget,Instant::now()+budget.page_timeout,&CancellationToken::new(),move |conn| Box::pin(async move {
            Ok(sqlx::query_as("SELECT g.file_path,g.artifact_version FROM artifact_file_staging_guard g WHERE NOT EXISTS(SELECT 1 FROM artifact_version v WHERE v.id=g.artifact_version OR v.file_path=g.file_path) ORDER BY g.artifact_version,g.file_path LIMIT $1")
                .bind(limit.max(1)).fetch_all(conn).await?)
        })).await
    }

    pub async fn release_orphaned_staging_bounded(
        pool: &PgPool,
        path: String,
        version: i64,
        budget: &MaintenanceBudget,
    ) -> MaintenanceResult<()> {
        let probe_path = path.clone();
        maintenance_admission::run(pool,budget,Instant::now()+budget.page_timeout,&CancellationToken::new(),
            move |conn| Box::pin(async move {
                Ok(sqlx::query_scalar::<_,String>("SELECT xmin::text FROM artifact_file_staging_guard WHERE file_path=$1 AND artifact_version=$2")
                    .bind(probe_path).bind(version).fetch_all(conn).await?)
            }),
            move |conn,candidates| Box::pin(async move {
                let current: Option<i64> = sqlx::query_scalar("SELECT artifact_version FROM artifact_file_staging_guard g WHERE file_path=$1 AND artifact_version=$2 AND xmin::text=$3 AND NOT EXISTS(SELECT 1 FROM artifact_version v WHERE v.id=g.artifact_version OR v.file_path=g.file_path) FOR UPDATE")
                    .bind(&path).bind(version).bind(&candidates[0]).fetch_optional(&mut *conn).await?;
                if current.is_none() { return Ok(((),0)); }
                super::file_evidence_retention::FileEvidenceRetentionRepository::staging_removed(conn,&path,version).await?;
                let changed = sqlx::query("DELETE FROM artifact_file_staging_guard WHERE file_path=$1 AND artifact_version=$2")
                    .bind(path).bind(version).execute(conn).await?.rows_affected();
                Ok(((),changed))
            }),
        ).await
    }

    pub async fn delete_artifact_version(pool: &PgPool, id: i64) -> Result<bool> {
        let result = sqlx::query("DELETE FROM artifact_version WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// An outside candidate is not deletion authority. Recheck the current
    /// policy/version frontier only after the GC transaction entered the graph
    /// exclusively, before claiming physical cleanup or touching any file.
    pub async fn claim_expired_artifact_version(
        connection: &mut sqlx::PgConnection,
        id: i64,
    ) -> Result<Option<ExpiredArtifactVersion>> {
        let held: bool = sqlx::query_scalar("SELECT file_evidence_graph_held(true)")
            .fetch_one(&mut *connection)
            .await?;
        if !held {
            return Err(crate::Error::invalid_state(
                "Artifact reclamation must enter exclusive graph admission at transaction entry",
            ));
        }
        let artifact: Option<i64> =
            sqlx::query_scalar("SELECT artifact FROM artifact_version WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
        let Some(artifact) = artifact else {
            return Ok(None);
        };
        if super::artifact::ArtifactRepository::find_by_id_for_update(connection, artifact)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        Ok(sqlx::query_as(&format!(
            "SELECT av.id,av.artifact,av.version,av.file_path,av.body_state,av.object_key,av.provider_version \
             FROM artifact_version av JOIN artifact a ON a.id=av.artifact \
             WHERE av.id=$1 AND av.artifact=$2 AND file_evidence_version_eligible(av.id) AND {} FOR UPDATE OF av",
            expired_artifact_version_predicate()
        )).bind(id).bind(artifact).fetch_optional(connection).await?)
    }

    pub async fn refresh_or_delete_artifact_metadata(
        pool: &PgPool,
        artifact_id: i64,
    ) -> Result<bool> {
        let mut conn = pool.acquire().await?;
        Self::refresh_or_delete_artifact_metadata_with_conn(&mut conn, artifact_id)
            .await
            .map(|(deleted, _)| deleted)
    }

    pub(super) async fn refresh_or_delete_artifact_metadata_with_conn(
        conn: &mut PgConnection,
        artifact_id: i64,
    ) -> Result<(bool, u64)> {
        let refreshed = sqlx::query(
            "WITH latest AS (
                SELECT size_bytes, content_type
                 FROM artifact_version
                 WHERE artifact = $1
                   AND (body_state IS NULL OR body_state = 'ready')
                 ORDER BY version DESC
                LIMIT 1
             )
             UPDATE artifact a
             SET size_bytes = latest.size_bytes,
                 content_type = COALESCE(latest.content_type, a.content_type),
                 updated = NOW()
             FROM latest
             WHERE a.id = $1",
        )
        .bind(artifact_id)
        .execute(&mut *conn)
        .await?;

        if refreshed.rows_affected() > 0 {
            return Ok((false, refreshed.rows_affected()));
        }

        let deleted = sqlx::query(
            "DELETE FROM artifact a
             WHERE a.id = $1
               AND NOT EXISTS (
                   SELECT 1 FROM artifact_version av WHERE av.artifact = a.id
               )
               AND (
                   a.data IS NULL
                   OR a.data = 'null'::jsonb
                   OR a.data = '[]'::jsonb
                   OR a.data = '{}'::jsonb
               )",
        )
        .bind(artifact_id)
        .execute(conn)
        .await?;

        Ok((deleted.rows_affected() > 0, deleted.rows_affected()))
    }

    pub async fn refresh_or_delete_artifact_metadata_bounded(
        pool: &PgPool,
        artifact_id: i64,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<bool> {
        maintenance_admission::run(
            pool,
            budget,
            deadline,
            cancel,
            move |conn| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar::<_, String>(
                        "SELECT xmin::text FROM artifact WHERE id=$1",
                    )
                    .bind(artifact_id)
                    .fetch_all(conn)
                    .await?)
                })
            },
            move |conn, candidates| {
                Box::pin(async move {
                    let current: Option<i64> = sqlx::query_scalar(
                        "SELECT id FROM artifact WHERE id=$1 AND xmin::text=$2 FOR UPDATE",
                    )
                    .bind(artifact_id)
                    .bind(&candidates[0])
                    .fetch_optional(&mut *conn)
                    .await?;
                    if current.is_none() {
                        return Ok((false, 0));
                    }
                    // Refresh reads the current ready-version frontier. The DELETE
                    // predicate and its triggers recheck emptiness, data and pins.
                    Self::refresh_or_delete_artifact_metadata_with_conn(conn, artifact_id).await
                })
            },
        )
        .await
    }

    pub async fn stuck_runtime_snapshots(
        pool: &PgPool,
        execution_stale_seconds: u64,
        queue_stale_seconds: u64,
    ) -> Result<Vec<StuckRuntimeSnapshot>> {
        let execution_cutoff = seconds_ago(execution_stale_seconds);
        let queue_cutoff = seconds_ago(queue_stale_seconds);
        let mut snapshots = Vec::new();

        let execution_rows = sqlx::query(
            "SELECT status::TEXT AS status, COUNT(*)::BIGINT AS count, MIN(updated) AS oldest
             FROM execution
             WHERE status IN ('requested', 'scheduling', 'scheduled', 'running', 'canceling')
               AND updated < $1
             GROUP BY status
             ORDER BY MIN(updated) ASC",
        )
        .bind(execution_cutoff)
        .fetch_all(pool)
        .await?;

        for row in execution_rows {
            snapshots.push(StuckRuntimeSnapshot {
                kind: "execution",
                status: row.get("status"),
                count: row.get("count"),
                oldest: row.get("oldest"),
            });
        }

        let queue_item_rows = sqlx::query(
            "SELECT status::TEXT AS status, COUNT(*)::BIGINT AS count, MIN(lease_expires_at) AS oldest
             FROM work_queue_item
             WHERE status = 'leased'
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < $1
             GROUP BY status
             ORDER BY MIN(lease_expires_at) ASC",
        )
        .bind(queue_cutoff)
        .fetch_all(pool)
        .await?;

        for row in queue_item_rows {
            snapshots.push(StuckRuntimeSnapshot {
                kind: "work_queue_item",
                status: row.get("status"),
                count: row.get("count"),
                oldest: row.get("oldest"),
            });
        }

        let dispatch_rows = sqlx::query(
            "SELECT status::TEXT AS status, COUNT(*)::BIGINT AS count, MIN(updated) AS oldest
             FROM work_queue_dispatch
             WHERE status IN ('leased', 'dispatched')
               AND updated < $1
             GROUP BY status
             ORDER BY MIN(updated) ASC",
        )
        .bind(queue_cutoff)
        .fetch_all(pool)
        .await?;

        for row in dispatch_rows {
            snapshots.push(StuckRuntimeSnapshot {
                kind: "work_queue_dispatch",
                status: row.get("status"),
                count: row.get("count"),
                oldest: row.get("oldest"),
            });
        }

        Ok(snapshots)
    }

    pub async fn alert_recently_emitted(
        pool: &PgPool,
        correlation_id: &str,
        cooldown_seconds: u64,
    ) -> Result<bool> {
        let cutoff = seconds_ago(cooldown_seconds);
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1
                 FROM event
                 WHERE trigger_ref = 'core.alert'
                   AND created > $1
                   AND payload ->> 'correlation_id' = $2
             )",
        )
        .bind(cutoff)
        .bind(correlation_id)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn find_stale_execution_candidates(
        pool: &PgPool,
        stale_seconds: u64,
        limit: i64,
    ) -> Result<Vec<StaleExecutionCandidate>> {
        let cutoff = seconds_ago(stale_seconds);
        sqlx::query_as::<_, StaleExecutionCandidate>(
            "SELECT e.id, e.action, e.action_ref, e.status::TEXT AS status, e.updated, e.worker
             FROM execution e
             LEFT JOIN worker w ON w.id = e.worker
             LEFT JOIN execution_reschedule_state rs ON rs.execution_id = e.id
             WHERE (
                   e.status IN ('scheduling', 'scheduled', 'canceling')
                   AND e.updated < $1
                 )
                OR (
                   e.status = 'requested'
                   AND e.updated < $1
                   AND (
                       rs.last_attempt_at IS NULL
                       OR rs.last_attempt_at < $1
                   )
                )
                OR (
                    e.status = 'running'
                    AND e.updated < $1
                    AND (
                        e.worker IS NULL
                        OR w.id IS NULL
                        OR w.status IN ('inactive', 'error')
                        OR w.last_heartbeat IS NULL
                        OR w.last_heartbeat < $1
                    )
                )
             ORDER BY e.updated ASC, e.id ASC
             LIMIT $2",
        )
        .bind(cutoff)
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn find_requested_executions_for_reschedule(
        pool: &PgPool,
        grace_seconds: u64,
        max_attempts: u32,
        limit: i64,
    ) -> Result<Vec<ExecutionRescheduleCandidate>> {
        let cutoff = seconds_ago(grace_seconds);
        sqlx::query_as::<_, ExecutionRescheduleCandidate>(
            "SELECT
                 e.id AS execution_id,
                 e.action AS action_id,
                 e.action_ref,
                 e.parent AS parent_id,
                 e.enforcement AS enforcement_id,
                 e.config,
                 e.pack_release AS release_id,
                 e.pack_release_digest AS release_digest,
                 COALESCE(rs.attempt_count, 0) AS attempt_count,
                 rs.last_attempt_at
             FROM execution e
             LEFT JOIN execution_reschedule_state rs ON rs.execution_id = e.id
             WHERE e.status = 'requested'
               AND e.updated < $1
               AND (COALESCE(rs.attempt_count, 0) < $2 OR shared_resource_waiter_ready(e.id))
               AND (rs.last_attempt_at IS NULL OR rs.last_attempt_at < $1)
               AND NOT EXISTS (
                    SELECT 1
                    FROM execution_admission_entry admission
                    WHERE admission.execution_id = e.id
                       AND admission.status = 'queued' AND NOT shared_resource_waiter_ready(e.id)
               )
             ORDER BY COALESCE(rs.last_attempt_at, e.updated) ASC, e.updated ASC, e.id ASC
             LIMIT $3",
        )
        .bind(cutoff)
        .bind(max_attempts as i32)
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn mark_execution_reschedule_attempt<'e, E>(
        executor: E,
        execution_id: Id,
        source: &str,
        reason: &str,
        max_attempts: u32,
        grace_seconds: u64,
        ignore_backoff: bool,
    ) -> Result<Option<ExecutionRescheduleAttempt>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let cutoff = seconds_ago(grace_seconds);
        sqlx::query_as::<_, ExecutionRescheduleAttempt>(
            "WITH candidate AS (
                 SELECT e.id, e.action, e.action_ref, e.parent, e.enforcement, e.config,
                        e.pack_release, e.pack_release_digest
                 FROM execution e
                 LEFT JOIN execution_reschedule_state rs ON rs.execution_id = e.id
                 WHERE e.id = $1
                   AND e.status = 'requested'
                   AND (COALESCE(rs.attempt_count, 0) < $2 OR shared_resource_waiter_ready(e.id))
                   AND ($6 OR rs.last_attempt_at IS NULL OR rs.last_attempt_at < $3)
                   AND NOT EXISTS (
                        SELECT 1
                        FROM execution_admission_entry admission
                        WHERE admission.execution_id = e.id
                           AND admission.status = 'queued' AND NOT shared_resource_waiter_ready(e.id)
                   )
                 FOR UPDATE OF e
             ),
             upserted AS (
                 INSERT INTO execution_reschedule_state (
                     execution_id, attempt_count, last_attempt_at, last_reason, last_source
                 )
                 SELECT id, 1, NOW(), $4, $5
                 FROM candidate
                 ON CONFLICT (execution_id) DO UPDATE
                 SET attempt_count = execution_reschedule_state.attempt_count + 1,
                     last_attempt_at = NOW(),
                     last_reason = EXCLUDED.last_reason,
                     last_source = EXCLUDED.last_source,
                     updated = NOW()
                 RETURNING execution_id, attempt_count, last_attempt_at, last_reason, last_source
             )
             SELECT
                 c.id AS execution_id,
                 c.action AS action_id,
                 c.action_ref,
                 c.parent AS parent_id,
                 c.enforcement AS enforcement_id,
                 c.config,
                 c.pack_release AS release_id,
                 c.pack_release_digest AS release_digest,
                 u.attempt_count,
                 u.last_attempt_at,
                 u.last_reason,
                 u.last_source
             FROM candidate c
             JOIN upserted u ON u.execution_id = c.id",
        )
        .bind(execution_id)
        .bind(max_attempts as i32)
        .bind(cutoff)
        .bind(reason)
        .bind(source)
        .bind(ignore_backoff)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn remediate_work_queue_state(
        pool: &PgPool,
        stale_seconds: u64,
    ) -> Result<QueueRemediationResult> {
        let cutoff = seconds_ago(stale_seconds);
        let row = sqlx::query(
            "WITH active_dispatch AS (
                SELECT
                    d.id,
                    d.execution,
                    d.queue,
                    CASE
                        WHEN e.id IS NULL OR e.status IN ('cancelled', 'canceling', 'abandoned')
                            THEN 'cancelled'::work_queue_dispatch_status_enum
                        ELSE 'failed'::work_queue_dispatch_status_enum
                    END AS new_dispatch_status,
                    COALESCE((q.config->'dispatch'->>'retry_limit')::INT, 0) AS retry_limit
                FROM work_queue_dispatch d
                LEFT JOIN execution e ON e.id = d.execution
                LEFT JOIN work_queue q ON q.id = d.queue
                WHERE d.status IN ('leased', 'dispatched')
                  AND (
                    d.updated < $1
                    OR e.id IS NULL
                    OR e.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
                  )
             ),
             updated_items AS (
                UPDATE work_queue_item item
                SET status = CASE
                        WHEN item.attempt_count < active_dispatch.retry_limit
                            THEN 'retry'::work_queue_item_status_enum
                        ELSE 'failed'::work_queue_item_status_enum
                    END,
                    leased_execution = NULL,
                    lease_token = NULL,
                    lease_expires_at = NULL,
                    last_error = jsonb_build_object(
                        'code', 'supervisor_stale_dispatch_reconciled',
                        'message', 'Supervisor released stale queue dispatch lease',
                        'execution_id', active_dispatch.execution,
                        'dispatch_id', active_dispatch.id
                    ),
                    ack_summary = jsonb_build_object(
                        'status', 'retry',
                        'effective_status', CASE
                            WHEN item.attempt_count < active_dispatch.retry_limit
                                THEN 'retry'
                            ELSE 'failed'
                        END,
                        'corrected_by', 'attune-supervisor'
                    ),
                    updated = NOW()
                FROM active_dispatch
                WHERE item.status = 'leased'
                  AND item.leased_execution = active_dispatch.execution
                RETURNING item.id
             ),
             updated_dispatches AS (
                UPDATE work_queue_dispatch d
                SET status = active_dispatch.new_dispatch_status,
                    updated = NOW()
                FROM active_dispatch
                WHERE d.id = active_dispatch.id
                RETURNING d.id
             ),
             orphan_items AS (
                SELECT
                    item.id,
                    COALESCE((q.config->'dispatch'->>'retry_limit')::INT, 0) AS retry_limit
                FROM work_queue_item item
                JOIN work_queue q ON q.id = item.queue
                WHERE item.status = 'leased'
                  AND item.lease_expires_at IS NOT NULL
                  AND item.lease_expires_at < $1
                  AND NOT EXISTS (
                    SELECT 1
                    FROM work_queue_dispatch d
                    WHERE d.execution = item.leased_execution
                      AND d.status IN ('leased', 'dispatched')
                  )
             ),
             updated_orphan_items AS (
                UPDATE work_queue_item item
                SET status = CASE
                        WHEN item.attempt_count < orphan_items.retry_limit
                            THEN 'retry'::work_queue_item_status_enum
                        ELSE 'failed'::work_queue_item_status_enum
                    END,
                    leased_execution = NULL,
                    lease_token = NULL,
                    lease_expires_at = NULL,
                    last_error = jsonb_build_object(
                        'code', 'supervisor_stale_item_reconciled',
                        'message', 'Supervisor released stale queue item lease'
                    ),
                    ack_summary = jsonb_build_object(
                        'status', 'retry',
                        'effective_status', CASE
                            WHEN item.attempt_count < orphan_items.retry_limit
                                THEN 'retry'
                            ELSE 'failed'
                        END,
                        'corrected_by', 'attune-supervisor'
                    ),
                    updated = NOW()
                FROM orphan_items
                WHERE item.id = orphan_items.id
                RETURNING item.id
             )
             SELECT
                (SELECT COUNT(*)::BIGINT FROM updated_dispatches) AS dispatches_corrected,
                (
                    (SELECT COUNT(*)::BIGINT FROM updated_items)
                    + (SELECT COUNT(*)::BIGINT FROM updated_orphan_items)
                ) AS items_corrected",
        )
        .bind(cutoff)
        .fetch_one(pool)
        .await?;

        Ok(QueueRemediationResult {
            dispatches_corrected: row.get("dispatches_corrected"),
            items_corrected: row.get("items_corrected"),
        })
    }

    pub async fn remediate_admission_state(
        pool: &PgPool,
        stale_seconds: u64,
    ) -> Result<AdmissionRemediationResult> {
        let cutoff = seconds_ago(stale_seconds);
        let mut tx = pool.begin().await?;

        let doomed = sqlx::query(
            "SELECT entry.id, entry.state_id, entry.status
             FROM execution_admission_entry entry
             JOIN execution e ON e.id = entry.execution_id
             WHERE e.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
                OR (
                    entry.status = 'active'
                    AND entry.updated < $1
                    AND e.status IN ('requested', 'scheduling', 'scheduled', 'canceling')
                )",
        )
        .bind(cutoff)
        .fetch_all(&mut *tx)
        .await?;

        let mut entries_removed = 0_i64;
        let mut active_entries_removed = 0_i64;
        let mut state_ids = Vec::new();

        for row in doomed {
            let entry_id: Id = row.get("id");
            let state_id: Id = row.get("state_id");
            let status: String = row.get("status");
            let result = sqlx::query("DELETE FROM execution_admission_entry WHERE id = $1")
                .bind(entry_id)
                .execute(&mut *tx)
                .await?;
            if result.rows_affected() > 0 {
                entries_removed += 1;
                if status == "active" {
                    active_entries_removed += 1;
                    sqlx::query(
                        "UPDATE execution_admission_state
                         SET total_completed = total_completed + 1, updated = NOW()
                         WHERE id = $1",
                    )
                    .bind(state_id)
                    .execute(&mut *tx)
                    .await?;
                }
                if !state_ids.contains(&state_id) {
                    state_ids.push(state_id);
                }
            }
        }

        let mut promoted_execution_ids = Vec::new();
        for state_id in state_ids {
            loop {
                let row = sqlx::query(
                    "WITH state AS (
                        SELECT id, max_concurrent
                        FROM execution_admission_state
                        WHERE id = $1
                        FOR UPDATE
                     ),
                     capacity AS (
                        SELECT state.id, state.max_concurrent,
                               COUNT(entry.id) FILTER (WHERE entry.status = 'active') AS active_count
                        FROM state
                        LEFT JOIN execution_admission_entry entry ON entry.state_id = state.id
                        GROUP BY state.id, state.max_concurrent
                     ),
                     next_queued AS (
                        SELECT entry.id, entry.execution_id
                        FROM execution_admission_entry entry
                        JOIN capacity ON capacity.id = entry.state_id
                        WHERE entry.status = 'queued'
                          AND capacity.active_count < capacity.max_concurrent
                        ORDER BY entry.queue_order ASC, entry.id ASC
                        LIMIT 1
                     )
                     UPDATE execution_admission_entry entry
                     SET status = 'active', activated_at = NOW(), updated = NOW()
                     FROM next_queued
                      WHERE entry.id = next_queued.id
                        AND NOT EXISTS (SELECT 1 FROM execution_resource_admission resource WHERE resource.execution_id = entry.execution_id AND NOT resource.active)
                     RETURNING entry.execution_id",
                )
                .bind(state_id)
                .fetch_optional(&mut *tx)
                .await?;

                let Some(row) = row else {
                    break;
                };
                promoted_execution_ids.push(row.get("execution_id"));
            }
        }

        tx.commit().await?;
        Ok(AdmissionRemediationResult {
            entries_removed,
            active_entries_removed,
            promoted_execution_ids,
        })
    }

    pub async fn remediate_workflow_state(
        pool: &PgPool,
        stale_seconds: u64,
    ) -> Result<WorkflowRemediationResult> {
        let cutoff = seconds_ago(stale_seconds);
        let terminal_sync = sqlx::query(
            "WITH corrected AS (
                UPDATE workflow_execution wf
                SET status = parent.status,
                    paused = false,
                    pause_reason = NULL,
                    error_message = COALESCE(
                        wf.error_message,
                        'Supervisor synchronized workflow state from terminal parent execution'
                    ),
                    updated = NOW()
                FROM execution parent
                WHERE wf.execution = parent.id
                  AND wf.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
                  AND parent.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
                  AND wf.updated < $1
                RETURNING wf.id
             )
             SELECT COUNT(*)::BIGINT AS count FROM corrected",
        )
        .bind(cutoff)
        .fetch_one(pool)
        .await?
        .get::<i64, _>("count");

        let parent_rows = sqlx::query(
            "WITH candidate AS (
                SELECT parent.id
                FROM workflow_execution wf
                JOIN execution parent ON parent.id = wf.execution
                WHERE parent.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
                  AND wf.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
                  AND wf.updated < $1
                  AND EXISTS (
                    SELECT 1 FROM execution child
                    WHERE child.parent = parent.id
                      AND child.status IN ('failed', 'cancelled', 'timeout', 'abandoned')
                  )
                  AND NOT EXISTS (
                    SELECT 1 FROM execution child
                    WHERE child.parent = parent.id
                      AND child.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
                  )
             ),
             updated_parent AS (
                UPDATE execution e
                SET status = 'failed',
                    result = jsonb_build_object(
                        'error', 'Workflow failed because all child executions are terminal and at least one child failed or was abandoned',
                        'corrected_by', 'attune-supervisor',
                        'previous_status', e.status,
                        'corrected_at', NOW()
                    ),
                    updated = NOW()
                FROM candidate
                WHERE e.id = candidate.id
                RETURNING e.id
             ),
             updated_wf AS (
                UPDATE workflow_execution wf
                SET status = 'failed',
                    paused = false,
                    pause_reason = NULL,
                    error_message = 'Supervisor failed workflow after terminal failed/abandoned children left it stale',
                    updated = NOW()
                FROM updated_parent
                WHERE wf.execution = updated_parent.id
                RETURNING wf.id
             )
             SELECT id FROM updated_parent",
        )
        .bind(cutoff)
        .fetch_all(pool)
        .await?;

        Ok(WorkflowRemediationResult {
            workflow_executions_corrected: terminal_sync + parent_rows.len() as i64,
            parent_executions_corrected: parent_rows.into_iter().map(|row| row.get("id")).collect(),
        })
    }
}

// This is an expiration prefilter, not a bounded raw scan. Keep evidence
// evaluation outside the materialized relation; an output LIMIT is not a cursor.
fn expired_artifact_prefilter() -> String {
    format!("WITH expired AS MATERIALIZED (
        SELECT av.id,av.artifact,av.version,av.file_path,av.body_state,av.object_key,av.provider_version,av.created
        FROM artifact_version av JOIN artifact a ON a.id=av.artifact WHERE {}
    )", expired_artifact_version_predicate())
}

fn expired_artifact_version_predicate() -> &'static str {
    "(av.body_state IS NULL OR av.body_state = 'ready') AND (
        (a.retention_policy = 'days' AND av.created < NOW() - make_interval(days => a.retention_limit))
        OR (a.retention_policy = 'hours' AND av.created < NOW() - make_interval(hours => a.retention_limit))
        OR (a.retention_policy = 'minutes' AND av.created < NOW() - make_interval(mins => a.retention_limit))
        OR (
            a.retention_policy = 'versions'
            AND a.retention_limit > 0
            AND NOT EXISTS (
                SELECT 1 FROM log_stream active_stream
                WHERE active_stream.artifact_version = av.id
                  AND active_stream.backend <> 'shared_file' AND NOT active_stream.sealed
            )
            AND EXISTS (
                SELECT 1 FROM artifact_version newer
                WHERE newer.artifact = av.artifact
                  AND (newer.body_state IS NULL OR newer.body_state = 'ready')
                  AND newer.version > av.version
                ORDER BY newer.version DESC
                OFFSET GREATEST(a.retention_limit::bigint - 1, 0) LIMIT 1
            )
        )
    )"
}

fn seconds_ago(seconds: u64) -> DateTime<Utc> {
    let seconds = seconds.min(i64::MAX as u64) as i64;
    Utc::now() - Duration::seconds(seconds)
}
