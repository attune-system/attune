//! Durable cadences under the supervisor's session advisory leader lock.
//! An attempt advances its retry deadline before work starts. A crash cannot
//! turn an overdue job into a busy loop; source catalogs/ledgers own progress.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use utoipa::ToSchema;

use super::SummaryKind;
use crate::{Error, Result};

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct SummaryBacklogStatus {
    pub kind: SummaryKind,
    pub notifications_at_least: i64,
    pub count_exact: bool,
    pub oldest_observed_notification: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceJob {
    Partition,
    Summary,
    Retention,
}

impl MaintenanceJob {
    pub const ALL: [Self; 3] = [Self::Partition, Self::Summary, Self::Retention];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Partition => "partition",
            Self::Summary => "summary",
            Self::Retention => "retention",
        }
    }
}

impl TryFrom<String> for MaintenanceJob {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        match value.as_str() {
            "partition" => Ok(Self::Partition),
            "summary" => Ok(Self::Summary),
            "retention" => Ok(Self::Retention),
            _ => Err(Error::Validation("unknown native maintenance job".into())),
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, ToSchema)]
pub struct MaintenanceScheduleStatus {
    #[sqlx(try_from = "String")]
    pub job: MaintenanceJob,
    pub next_due: DateTime<Utc>,
    pub last_success: Option<DateTime<Utc>>,
}

impl MaintenanceScheduleStatus {
    pub fn due_at(&self, now: DateTime<Utc>) -> bool {
        self.next_due <= now
    }
}

pub struct ScheduleRepository;

impl ScheduleRepository {
    /// Cap the indexed notification probe per kind and bound the metadata query.
    /// Do not scan/count the entire append log during a maintenance tick.
    pub async fn summary_backlog_status(
        pool: &PgPool,
        cap: i64,
        timeout_milliseconds: u64,
    ) -> Result<Vec<SummaryBacklogStatus>> {
        if cap <= 0
            || cap == i64::MAX
            || timeout_milliseconds == 0
            || timeout_milliseconds > i32::MAX as u64
        {
            return Err(Error::Validation("invalid native status budget".into()));
        }
        let mut tx = pool.begin().await?;
        if let Err(error) = sqlx::query("SELECT set_config('statement_timeout', $1, true)")
            .bind(format!("{timeout_milliseconds}ms"))
            .execute(&mut *tx)
            .await
        {
            tx.rollback().await?;
            return Err(error.into());
        }
        let rows = sqlx::query_as(
            "SELECT k.kind, d.notifications_at_least, d.notifications_at_least <= $1 AS count_exact,
                 d.oldest_observed_notification
             FROM unnest(enum_range(NULL::native_summary_kind)) AS k(kind)
             CROSS JOIN LATERAL (
                 SELECT count(*)::bigint AS notifications_at_least, min(n.created) AS oldest_observed_notification
                 FROM (SELECT created FROM native_summary_invalidation WHERE kind = k.kind
                       ORDER BY bucket, id LIMIT $1 + 1) n
             ) d ORDER BY k.kind",
        ).bind(cap).fetch_all(&mut *tx).await;
        match rows {
            Ok(rows) => {
                tx.commit().await?;
                Ok(rows)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error.into())
            }
        }
    }
    /// Idempotent fallback for missing rows. Never reset another leader's cadence.
    pub async fn ensure(pool: &PgPool, now: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "INSERT INTO native_maintenance_schedule (job, next_due)
             SELECT unnest(ARRAY['partition', 'summary', 'retention']::text[]), $1
             ON CONFLICT (job) DO NOTHING",
        )
        .bind(now)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Three fixed job rows, suitable for the protected operator status API.
    pub async fn status(pool: &PgPool) -> Result<Vec<MaintenanceScheduleStatus>> {
        Ok(sqlx::query_as(
            "SELECT job, next_due, last_success FROM native_maintenance_schedule ORDER BY job",
        )
        .fetch_all(pool)
        .await?)
    }

    /// Call only while holding the existing supervisor leader lock. Startup may
    /// force partition reconciliation; summary and retention stay cadence-bound.
    pub async fn begin_attempt(
        pool: &PgPool,
        job: MaintenanceJob,
        now: DateTime<Utc>,
        interval_seconds: u64,
        force: bool,
    ) -> Result<bool> {
        let next_due = next_deadline(now, interval_seconds, false)?;
        Ok(sqlx::query(
            "UPDATE native_maintenance_schedule SET next_due = $3
             WHERE job = $1 AND (next_due <= $2 OR $4)",
        )
        .bind(job.name())
        .bind(now)
        .bind(next_due)
        .bind(force)
        .execute(pool)
        .await?
        .rows_affected()
            == 1)
    }

    /// Successful bounded work earns the normal cadence, even when backfill is
    /// still pending. Failed/cancelled work keeps last_success and retries later.
    pub async fn finish_attempt(
        pool: &PgPool,
        job: MaintenanceJob,
        now: DateTime<Utc>,
        interval_seconds: u64,
        success: bool,
    ) -> Result<()> {
        let next_due = next_deadline(now, interval_seconds, success)?;
        sqlx::query(
            "UPDATE native_maintenance_schedule
             SET next_due = $2, last_success = CASE WHEN $3 THEN $4 ELSE last_success END
             WHERE job = $1",
        )
        .bind(job.name())
        .bind(next_due)
        .bind(success)
        .bind(now)
        .execute(pool)
        .await?;
        Ok(())
    }
}

pub fn next_deadline(
    now: DateTime<Utc>,
    interval_seconds: u64,
    success: bool,
) -> Result<DateTime<Utc>> {
    let seconds = i64::try_from(interval_seconds)
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            Error::Validation("maintenance interval must be a positive BIGINT".into())
        })?;
    let seconds = if success { seconds } else { seconds.min(60) };
    Duration::try_seconds(seconds)
        .and_then(|delta| now.checked_add_signed(delta))
        .ok_or_else(|| Error::Validation("maintenance deadline exceeds timestamp range".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_cadences_and_bounded_failure_retry() {
        let now = DateTime::parse_from_rfc3339("2026-10-06T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            next_deadline(now, 300, true).unwrap(),
            now + Duration::minutes(5)
        );
        assert_eq!(
            next_deadline(now, 3600, true).unwrap(),
            now + Duration::hours(1)
        );
        assert_eq!(
            next_deadline(now, 3600, false).unwrap(),
            now + Duration::minutes(1)
        );
        assert_eq!(
            next_deadline(now, 5, false).unwrap(),
            now + Duration::seconds(5)
        );
        assert!(next_deadline(now, 0, true).is_err());
        assert!(next_deadline(now, u64::MAX, true).is_err());
    }

    #[test]
    fn job_text_is_a_checked_domain() {
        for job in MaintenanceJob::ALL {
            assert_eq!(
                MaintenanceJob::try_from(job.name().to_owned()).unwrap(),
                job
            );
        }
        assert!(MaintenanceJob::try_from("event; DROP TABLE event".to_owned()).is_err());
    }

    #[tokio::test]
    async fn durable_attempts_survive_restart_without_resetting_other_jobs() {
        use crate::{config::Config, test_database::TestDatabase};
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let pool = database.pool();
        let now = DateTime::parse_from_rfc3339("2040-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        sqlx::query("UPDATE native_maintenance_schedule SET next_due = $1, last_success = NULL")
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        assert!(ScheduleRepository::begin_attempt(
            pool,
            MaintenanceJob::Retention,
            now,
            3600,
            false
        )
        .await
        .unwrap());
        ScheduleRepository::finish_attempt(pool, MaintenanceJob::Retention, now, 3600, true)
            .await
            .unwrap();
        assert!(
            ScheduleRepository::begin_attempt(pool, MaintenanceJob::Summary, now, 300, false)
                .await
                .unwrap()
        );
        ScheduleRepository::finish_attempt(pool, MaintenanceJob::Summary, now, 300, true)
            .await
            .unwrap();
        let next = now + Duration::minutes(5);
        ScheduleRepository::ensure(pool, next).await.unwrap();
        assert!(!ScheduleRepository::begin_attempt(
            pool,
            MaintenanceJob::Retention,
            next,
            3600,
            false
        )
        .await
        .unwrap());
        assert!(
            ScheduleRepository::begin_attempt(pool, MaintenanceJob::Summary, next, 300, false)
                .await
                .unwrap()
        );
        // Simulated crash after begin. The next leader cannot reset/reclaim it
        // until the durable retry deadline. Its successful timestamp survives.
        ScheduleRepository::ensure(pool, next).await.unwrap();
        assert!(!ScheduleRepository::begin_attempt(
            pool,
            MaintenanceJob::Summary,
            next,
            300,
            false
        )
        .await
        .unwrap());
        let statuses = ScheduleRepository::status(pool).await.unwrap();
        let summary = statuses
            .iter()
            .find(|row| row.job == MaintenanceJob::Summary)
            .unwrap();
        assert_eq!(summary.last_success, Some(now));
        assert_eq!(summary.next_due, next + Duration::minutes(1));
        let retry = next + Duration::minutes(1);
        assert!(ScheduleRepository::begin_attempt(
            pool,
            MaintenanceJob::Summary,
            retry,
            300,
            false
        )
        .await
        .unwrap());
        ScheduleRepository::finish_attempt(pool, MaintenanceJob::Summary, retry, 300, false)
            .await
            .unwrap();
        let statuses = ScheduleRepository::status(pool).await.unwrap();
        assert_eq!(
            statuses
                .iter()
                .find(|row| row.job == MaintenanceJob::Summary)
                .unwrap()
                .last_success,
            Some(now)
        );
        let retention = statuses
            .iter()
            .find(|row| row.job == MaintenanceJob::Retention)
            .unwrap();
        assert_eq!(retention.last_success, Some(now));
        assert_eq!(retention.next_due, now + Duration::hours(1));
        assert!(!retention.due_at(retry));
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn summary_observation_reports_bounded_lower_bound() {
        use crate::{config::Config, test_database::TestDatabase};
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        // Each record represents a different committed producer transaction.
        // Multiple notifications for one producer/hour intentionally coalesce.
        for _ in 0..5 {
            sqlx::query(
                "INSERT INTO native_summary_invalidation(kind, bucket)
                         VALUES ('event_volume', date_trunc('hour', now(), 'UTC'))",
            )
            .execute(database.pool())
            .await
            .unwrap();
        }
        let rows = ScheduleRepository::summary_backlog_status(database.pool(), 2, 1000)
            .await
            .unwrap();
        assert_eq!(rows.len(), 4);
        let events = rows
            .iter()
            .find(|row| row.kind == SummaryKind::EventVolume)
            .unwrap();
        assert_eq!(events.notifications_at_least, 3);
        assert!(!events.count_exact);
        assert!(events.oldest_observed_notification.is_some());
        let worker = rows
            .iter()
            .find(|row| row.kind == SummaryKind::WorkerStatus)
            .unwrap();
        assert_eq!(worker.notifications_at_least, 0);
        assert!(worker.count_exact);
        database.cleanup().await.unwrap();
    }
}
