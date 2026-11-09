use chrono::{DateTime, Utc};
use sqlx::{Executor, FromRow, PgConnection, PgPool, Postgres};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::maintenance_admission::{
    self, MaintenanceBudget, MaintenanceOutcome, MaintenanceResult,
};
use super::pack_install::{PACK_INSTALL_ACTIVATION_TTL_SECS, PACK_INSTALL_ACTIVE_TTL_SECS};
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

struct DeletionCandidate {
    id: i64,
    xmin: String,
    progress_revision: i64,
    high_water: i64,
    next_cursor: i64,
    after: i64,
}

#[derive(Clone, Copy)]
struct DeletionRetry {
    revision: i64,
    after: i64,
    high_water: i64,
    next_cursor: i64,
    count: usize,
}

enum DeletionVisit {
    Row(DeletionCandidate),
    // An existing pass lost every remaining candidate. This is a cursor-only
    // correctness unit, never a synthetic ledger row or physical cleanup claim.
    EmptyPass { progress_revision: i64 },
}

impl ObjectMaintenanceRepository {
    pub async fn reserve_upload<'e, E>(executor: E, key: &str, kind: &str) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
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
        .fetch_optional(executor)
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
             FROM object_maintenance_ledger l WHERE state = 'uploading' AND updated < $1 \
             AND NOT EXISTS (SELECT 1 FROM file_evidence_put p JOIN file_evidence_owner o ON o.version=p.owner WHERE o.object_key=l.object_key) \
             AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g \
                             WHERE g.object_key = l.object_key AND g.state = 'issued' \
                               AND g.settle_until > clock_timestamp()) \
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
        // Native caller has just confirmed no object exists. Use the SAME
        // completion transaction, with an observation/version race check.
        Self::complete_delete_observation(pool, id, Some(observed_updated)).await
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
              AND NOT EXISTS (SELECT 1 FROM pack_install i WHERE i.candidate_object_key = l.object_key \
                  AND i.candidate_provider_version = l.provider_version \
                  AND ((i.status IN ('pending', 'running') AND i.started_at >= NOW() - ($3::bigint * INTERVAL '1 second')) \
                    OR (i.status = 'activating' AND i.updated_at >= NOW() - ($4::bigint * INTERVAL '1 second')))) \
               AND NOT EXISTS (SELECT 1 FROM artifact_version v WHERE v.object_key = l.object_key AND v.provider_version = l.provider_version) \
               AND NOT EXISTS (SELECT 1 FROM log_segment s WHERE s.object_key = l.object_key AND s.provider_version = l.provider_version) \
               AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g \
                               WHERE g.object_key = l.object_key AND g.state = 'issued' \
                                 AND g.settle_until > clock_timestamp())",
        )
        .bind(eligible_at)
        .bind(upload_cutoff)
        .bind(PACK_INSTALL_ACTIVE_TTL_SECS)
        .bind(PACK_INSTALL_ACTIVATION_TTL_SECS)
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
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx)
            .await?;
        let rows = Self::claim_deletions_with_conn(&mut tx, retry_cutoff, limit, None).await?;
        tx.commit().await?;
        Ok(rows)
    }

    async fn claim_deletions_with_conn(
        conn: &mut PgConnection,
        retry_cutoff: DateTime<Utc>,
        limit: i64,
        observed: Option<Vec<(i64, String)>>,
    ) -> Result<Vec<ObjectLedgerEntry>> {
        let (ids, revisions): (Option<Vec<_>>, Option<Vec<_>>) = match observed {
            Some(rows) => {
                let (ids, revisions) = rows.into_iter().unzip();
                (Some(ids), Some(revisions))
            }
            None => (None, None),
        };
        let rows = sqlx::query_as(
            "WITH candidates AS ( \
                  SELECT id FROM object_maintenance_ledger \
                  WHERE ($5::bigint[] IS NULL OR (id,xmin::text) IN (SELECT * FROM unnest($5::bigint[],$6::text[]))) \
                    AND NOT EXISTS(SELECT 1 FROM file_evidence_put p JOIN file_evidence_owner o ON o.version=p.owner WHERE o.object_key=object_maintenance_ledger.object_key) \
                    AND NOT object_log_close_object_pinned(object_key, provider_version) \
                    AND NOT EXISTS (SELECT 1 FROM artifact_upload_grant g \
                                   WHERE g.object_key = object_maintenance_ledger.object_key \
                                     AND g.state = 'issued' \
                                     AND g.settle_until > clock_timestamp()) \
                   AND ((state = 'deleting' AND updated < $1) OR ( \
                   state = 'deletion_pending' AND eligible_at < $1 \
                   AND NOT EXISTS (SELECT 1 FROM pack_release r \
                                   WHERE r.object_key = object_maintenance_ledger.object_key \
                                      AND r.provider_version = object_maintenance_ledger.provider_version) \
                    AND NOT EXISTS (SELECT 1 FROM pack_install i \
                        WHERE i.candidate_object_key = object_maintenance_ledger.object_key \
                          AND i.candidate_provider_version = object_maintenance_ledger.provider_version \
                          AND ((i.status IN ('pending', 'running') AND i.started_at >= NOW() - ($3::bigint * INTERVAL '1 second')) \
                            OR (i.status = 'activating' AND i.updated_at >= NOW() - ($4::bigint * INTERVAL '1 second')))) \
                   AND NOT EXISTS (SELECT 1 FROM artifact_version v \
                                   WHERE v.object_key = object_maintenance_ledger.object_key \
                                     AND v.provider_version = object_maintenance_ledger.provider_version) \
                   AND NOT EXISTS (SELECT 1 FROM log_segment s \
                                   WHERE s.object_key = object_maintenance_ledger.object_key \
                                      AND s.provider_version = object_maintenance_ledger.provider_version))) \
                 ORDER BY eligible_at NULLS LAST, updated, id \
                 FOR UPDATE SKIP LOCKED LIMIT $2 \
             ) \
             UPDATE object_maintenance_ledger l SET state = 'deleting', attempts = attempts + 1, updated = NOW() \
             FROM candidates c WHERE l.id = c.id \
             RETURNING l.id, l.object_key, l.provider_version, l.object_kind, l.size_bytes, l.state, l.attempts, l.updated",
        )
        .bind(retry_cutoff)
        .bind(limit.max(1))
        .bind(PACK_INSTALL_ACTIVE_TTL_SECS)
        .bind(PACK_INSTALL_ACTIVATION_TTL_SECS)
        .bind(ids)
        .bind(revisions)
        .fetch_all(conn)
        .await?;
        Ok(rows)
    }

    pub async fn claim_deletions_bounded(
        pool: &PgPool,
        retry_cutoff: DateTime<Utc>,
        limit: i64,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<Vec<ObjectLedgerEntry>> {
        // Set only after admission and the progress revision check, immediately
        // before claiming. A discovery/admission failure is not a failed visit.
        let attempted = std::sync::Arc::new(std::sync::Mutex::new(None));
        let attempted_ref = attempted.clone();
        let mut report = maintenance_admission::run(pool, budget, deadline, cancel,
            move |conn| Box::pin(Self::deletion_candidates(conn,retry_cutoff,limit.clamp(1,256))),
            move |conn, candidates| Box::pin(async move {
                let raw_count = candidates.iter().filter(|visit| matches!(visit,DeletionVisit::Row(_))).count();
                let (progress_revision,next_cursor,high_water) = match &candidates[0] {
                    DeletionVisit::Row(first) => (first.progress_revision,first.next_cursor,first.high_water),
                    DeletionVisit::EmptyPass { progress_revision } => (*progress_revision,0,0),
                };
                let revision: i64 = sqlx::query_scalar("SELECT revision FROM graph_reclamation_progress WHERE phase='object_deletions' FOR UPDATE")
                    .fetch_one(&mut *conn).await?;
                if revision != progress_revision { return Ok(((Vec::new(),raw_count),0)); }
                if let DeletionVisit::Row(first) = &candidates[0] {
                    *attempted_ref.lock().unwrap() = Some(DeletionRetry {
                        revision, after:first.after, high_water, next_cursor, count:raw_count,
                    });
                }
                let observed: Vec<_> = candidates.into_iter().filter_map(|visit| match visit {
                    DeletionVisit::Row(candidate) => Some((candidate.id,candidate.xmin)),
                    DeletionVisit::EmptyPass { .. } => None,
                }).collect();
                // Every eligibility/pin check and exact xmin check runs after
                // admission. Ineligible visits advance only on this same commit.
                let rows = if raw_count==0 { Vec::new() } else {
                    Self::claim_deletions_with_conn(conn, retry_cutoff, limit.clamp(1,256), Some(observed.clone())).await?
                };
                // An xmin race is not proof that a previously failed row became
                // ineligible. Keep its retry age until a later clean visit.
                let changed_identity = if rows.len()<raw_count {
                    let (ids,revisions): (Vec<_>,Vec<_>) = observed.into_iter().unzip();
                    sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM object_maintenance_ledger l JOIN unnest($1::bigint[],$2::text[]) seen(id,revision) ON l.id=seen.id WHERE l.xmin::text<>seen.revision AND ((l.state='deleting' AND l.updated<$3) OR (l.state='deletion_pending' AND l.eligible_at<$3)))")
                        .bind(ids).bind(revisions).bind(retry_cutoff).fetch_one(&mut *conn).await?
                } else { false };
                let changed = rows.len() as u64;
                sqlx::query("UPDATE graph_reclamation_progress SET cursor_id=$1,high_water=$2,revision=revision+1,last_attempt=clock_timestamp(),next_attempt=clock_timestamp(),last_progress=CASE WHEN $3 THEN clock_timestamp() ELSE last_progress END,probe_limit=CASE WHEN $1=0 AND NOT pass_deferred AND NOT $5 THEN 256 ELSE probe_limit END,deferred_since=CASE WHEN $5 THEN COALESCE(deferred_since,clock_timestamp()) WHEN $1=0 AND NOT pass_deferred THEN NULL ELSE deferred_since END,oversized_since=CASE WHEN $1=0 AND NOT pass_deferred AND NOT $5 THEN NULL ELSE oversized_since END,pass_deferred=CASE WHEN $1=0 THEN false ELSE pass_deferred OR $5 END WHERE phase='object_deletions' AND revision=$4")
                    .bind(next_cursor).bind(if next_cursor==0 {0} else {high_water}).bind(changed>0).bind(revision).bind(changed_identity).execute(conn).await?;
                Ok(((rows,raw_count),changed))
            }),
        ).await;
        let attempted = *attempted.lock().unwrap();
        if let Some(visit) = attempted.filter(|_| {
            report.rollback_acknowledged
                && !cancel.is_cancelled()
                && matches!(
                    report.outcome,
                    MaintenanceOutcome::DeadlineDeferred | MaintenanceOutcome::Failed
                )
        }) {
            // The claim rollback is acknowledged before this separately bounded
            // progress-only transaction. It cannot return claims or change the
            // ledger. Shrink a failed page without skipping its rows; only a
            // failed singleton advances, and the next frozen pass revisits it.
            let completion_end = deadline.min(Instant::now() + budget.statement_timeout);
            let completion =
                Self::deletion_failed_visit(pool, visit, budget, completion_end, cancel).await;
            if !matches!(
                completion.outcome,
                MaintenanceOutcome::Progress | MaintenanceOutcome::NoWork
            ) {
                report.outcome = completion.outcome;
                if completion.error.is_some() {
                    report.error = completion.error;
                }
            }
        }
        // The internal empty-pass sentinel is not an object candidate. Only
        // acknowledged commits can expose the corrected count or returned rows.
        let candidates = report
            .value
            .as_ref()
            .map_or(report.candidates, |(_, count)| *count);
        MaintenanceResult {
            outcome: report.outcome,
            value: report.value.map(|(rows, _)| rows),
            candidates,
            changed: report.changed,
            error: report.error,
            rollback_acknowledged: report.rollback_acknowledged,
        }
    }

    async fn deletion_failed_visit(
        pool: &PgPool,
        visit: DeletionRetry,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<()> {
        maintenance_admission::run(pool,budget,deadline,cancel,
            move |_| Box::pin(async move { Ok(vec![visit]) }),
            move |conn,visits| Box::pin(async move {
                let visit = visits[0];
                let singleton = visit.count==1;
                let cursor = if singleton { visit.next_cursor } else { visit.after };
                sqlx::query("UPDATE graph_reclamation_progress SET cursor_id=$1,high_water=$2,probe_limit=$3,revision=revision+1,last_attempt=clock_timestamp(),next_attempt=clock_timestamp(),oversized_since=CASE WHEN $4 THEN oversized_since ELSE COALESCE(oversized_since,clock_timestamp()) END,deferred_since=CASE WHEN $4 THEN COALESCE(deferred_since,clock_timestamp()) ELSE deferred_since END,pass_deferred=$1<>0 OR NOT $4 WHERE phase='object_deletions' AND revision=$5")
                    .bind(cursor).bind(if singleton && cursor==0 {0} else {visit.high_water})
                    .bind(if singleton {1} else {(visit.count as i64 / 2).max(1)})
                    .bind(singleton).bind(visit.revision).execute(conn).await?;
                Ok(((),0))
            }),
        ).await
    }

    /// Advisory scheduling state, not an eligible-object count. Continue a
    /// bounded raw pass promptly rather than waiting for the normal cadence.
    pub async fn deletion_scan_pending(pool: &PgPool) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT cursor_id>0 OR high_water>0 OR pass_deferred OR deferred_since IS NOT NULL OR oversized_since IS NOT NULL FROM graph_reclamation_progress WHERE phase='object_deletions'")
            .fetch_one(pool).await?)
    }

    /// Oldest unresolved failed claim, including over-budget multi-row probes.
    /// Cheap followers cannot clear this age before a complete clean pass.
    pub async fn deletion_deferred_since(pool: &PgPool) -> Result<Option<DateTime<Utc>>> {
        Ok(sqlx::query_scalar("SELECT LEAST(deferred_since,oversized_since) FROM graph_reclamation_progress WHERE phase='object_deletions'")
            .fetch_one(pool).await?)
    }

    async fn deletion_candidates(
        conn: &mut PgConnection,
        retry_cutoff: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<DeletionVisit>> {
        let (after,mut high_water,revision,probe_limit,deferred): (i64,i64,i64,i64,bool) = sqlx::query_as("SELECT cursor_id,high_water,revision,probe_limit,pass_deferred OR deferred_since IS NOT NULL OR oversized_since IS NOT NULL FROM graph_reclamation_progress WHERE phase='object_deletions'")
            .fetch_one(&mut *conn).await?;
        let pass_pending = after > 0 || high_water > 0 || deferred;
        let limit = limit.min(probe_limit);
        // Finish the frozen pass before wrapping, so a dirty pass cannot clear
        // failure age. Continuous insertion cannot hide failed older rows.
        if high_water == 0 {
            high_water =
                sqlx::query_scalar("SELECT COALESCE(max(id),0) FROM object_maintenance_ledger")
                    .fetch_one(&mut *conn)
                    .await?;
        }
        let rows: Vec<(i64,String)> = sqlx::query_as("SELECT id,xmin::text FROM object_maintenance_ledger WHERE id>$1 AND id<=$2 AND ((state='deleting' AND updated<$3) OR (state='deletion_pending' AND eligible_at<$3)) ORDER BY id LIMIT $4")
                .bind(after).bind(high_water).bind(retry_cutoff).bind(limit).fetch_all(&mut *conn).await?;
        if !rows.is_empty() {
            let last = rows.last().unwrap().0;
            let next_cursor = if rows.len() < limit as usize || last >= high_water {
                0
            } else {
                last
            };
            return Ok(rows
                .into_iter()
                .map(|(id, xmin)| {
                    DeletionVisit::Row(DeletionCandidate {
                        id,
                        xmin,
                        progress_revision: revision,
                        high_water,
                        next_cursor,
                        after,
                    })
                })
                .collect());
        }
        // Discovery itself never updates the cursor. The outer owner first
        // acknowledges discovery rollback, then commits this revision-checked
        // empty completion under the same bounded transaction rules as claims.
        Ok(if pass_pending {
            vec![DeletionVisit::EmptyPass {
                progress_revision: revision,
            }]
        } else {
            Vec::new()
        })
    }

    pub async fn exact_reference_exists(pool: &PgPool, key: &str, version: &str) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pack_release WHERE object_key = $1 AND provider_version = $2) \
             OR EXISTS (SELECT 1 FROM pack_install WHERE candidate_object_key = $1 AND candidate_provider_version = $2 \
                 AND ((status IN ('pending', 'running') AND started_at >= NOW() - ($3::bigint * INTERVAL '1 second')) \
                   OR (status = 'activating' AND updated_at >= NOW() - ($4::bigint * INTERVAL '1 second')))) \
             OR EXISTS (SELECT 1 FROM artifact_version WHERE object_key = $1 AND provider_version = $2) \
              OR EXISTS (SELECT 1 FROM log_segment WHERE object_key = $1 AND provider_version = $2) \
              OR EXISTS(SELECT 1 FROM file_evidence_put p JOIN file_evidence_owner o ON o.version=p.owner WHERE o.object_key=$1)",
        )
        .bind(key)
        .bind(version)
        .bind(PACK_INSTALL_ACTIVE_TTL_SECS)
        .bind(PACK_INSTALL_ACTIVATION_TTL_SECS)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn complete_delete(pool: &PgPool, id: i64) -> Result<bool> {
        Self::complete_delete_observation(pool, id, None).await
    }

    async fn complete_delete_observation(
        pool: &PgPool,
        id: i64,
        absent: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        let mut tx = pool.begin().await?;
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx)
            .await?;
        let deleted = Self::complete_delete_observation_with_conn(&mut tx, id, absent).await?;
        tx.commit().await?;
        Ok(deleted)
    }

    async fn complete_delete_observation_with_conn(
        conn: &mut PgConnection,
        id: i64,
        absent: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        let deleted:Option<(String,Option<String>)>=sqlx::query_as("DELETE FROM object_maintenance_ledger l WHERE id=$1 AND (($2::timestamptz IS NULL AND state='deleting') OR ($2 IS NOT NULL AND state='uploading' AND updated=$2 AND NOT EXISTS(SELECT 1 FROM artifact_upload_grant g WHERE g.object_key=l.object_key AND g.state='issued' AND g.settle_until>clock_timestamp()))) AND NOT EXISTS(SELECT 1 FROM file_evidence_put p JOIN file_evidence_owner o ON o.version=p.owner WHERE o.object_key=l.object_key) RETURNING object_key,provider_version")
            .bind(id).bind(absent).fetch_optional(&mut *conn).await?;
        if let Some((key, provider)) = &deleted {
            super::file_evidence_retention::FileEvidenceRetentionRepository::object_removed(
                conn,
                key,
                provider.as_deref(),
            )
            .await?;
        }
        Ok(deleted.is_some())
    }

    pub async fn complete_delete_bounded(
        pool: &PgPool,
        id: i64,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<bool> {
        Self::complete_delete_observation_bounded(pool, id, None, budget, deadline, cancel).await
    }

    pub async fn remove_stale_upload_bounded(
        pool: &PgPool,
        id: i64,
        observed_updated: DateTime<Utc>,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<bool> {
        Self::complete_delete_observation_bounded(
            pool,
            id,
            Some(observed_updated),
            budget,
            deadline,
            cancel,
        )
        .await
    }

    async fn complete_delete_observation_bounded(
        pool: &PgPool,
        id: i64,
        absent: Option<DateTime<Utc>>,
        budget: &MaintenanceBudget,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> MaintenanceResult<bool> {
        maintenance_admission::run(pool, budget, deadline, cancel,
            move |conn| Box::pin(async move {
                Ok(sqlx::query_scalar::<_, String>("SELECT xmin::text FROM object_maintenance_ledger WHERE id=$1 AND (($2::timestamptz IS NULL AND state='deleting') OR ($2 IS NOT NULL AND state='uploading' AND updated=$2))")
                    .bind(id).bind(absent).fetch_all(conn).await?)
            }),
            move |conn, candidates| Box::pin(async move {
                let current: Option<i64> = sqlx::query_scalar("SELECT id FROM object_maintenance_ledger WHERE id=$1 AND xmin::text=$2 FOR UPDATE")
                    .bind(id).bind(&candidates[0]).fetch_optional(&mut *conn).await?;
                if current.is_none() { return Ok((false,0)); }
                let deleted = Self::complete_delete_observation_with_conn(conn,id,absent).await?;
                Ok((deleted,u64::from(deleted)))
            }),
        ).await
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
