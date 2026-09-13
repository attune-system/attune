use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{Error, Result};

const ADMISSION_LOCK_KEY: &str = "execution_log_stream_admission";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExecutionLogStreamAdmission {
    Acquired {
        lease_id: Uuid,
        expires_at: DateTime<Utc>,
    },
    GlobalLimit,
    IdentityLimit,
}

pub struct ExecutionLogStreamLeaseRepository;

impl ExecutionLogStreamLeaseRepository {
    pub async fn acquire(
        pool: &PgPool,
        identity_id: i64,
        global_limit: usize,
        per_identity_limit: usize,
        lease_seconds: u64,
    ) -> Result<ExecutionLogStreamAdmission> {
        let global_limit = i64::try_from(global_limit)
            .map_err(|_| Error::validation("execution log stream global limit is too large"))?;
        let per_identity_limit = i64::try_from(per_identity_limit).map_err(|_| {
            Error::validation("execution log stream per-identity limit is too large")
        })?;
        let lease_seconds = i32::try_from(lease_seconds)
            .map_err(|_| Error::validation("execution log stream lease duration is too large"))?;
        let lease_id = Uuid::new_v4();
        let mut tx = pool.begin().await?;

        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(ADMISSION_LOCK_KEY)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM execution_log_stream_lease WHERE expires_at <= clock_timestamp()")
            .execute(&mut *tx)
            .await?;

        let (global_count, identity_count): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COUNT(*) FILTER (WHERE identity_id = $1) FROM execution_log_stream_lease",
        )
        .bind(identity_id)
        .fetch_one(&mut *tx)
        .await?;

        if global_count >= global_limit {
            tx.rollback().await?;
            return Ok(ExecutionLogStreamAdmission::GlobalLimit);
        }
        if identity_count >= per_identity_limit {
            tx.rollback().await?;
            return Ok(ExecutionLogStreamAdmission::IdentityLimit);
        }

        let expires_at = sqlx::query_scalar(
            "INSERT INTO execution_log_stream_lease (id, identity_id, expires_at) \
             VALUES ($1, $2, clock_timestamp() + make_interval(secs => $3)) RETURNING expires_at",
        )
        .bind(lease_id)
        .bind(identity_id)
        .bind(lease_seconds)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(ExecutionLogStreamAdmission::Acquired {
            lease_id,
            expires_at,
        })
    }

    pub async fn renew(pool: &PgPool, lease_id: Uuid, lease_seconds: u64) -> Result<bool> {
        let lease_seconds = i32::try_from(lease_seconds)
            .map_err(|_| Error::validation("execution log stream lease duration is too large"))?;
        let result = sqlx::query(
            "UPDATE execution_log_stream_lease \
             SET expires_at = clock_timestamp() + make_interval(secs => $2), renewed = clock_timestamp() \
             WHERE id = $1 AND expires_at > clock_timestamp()",
        )
        .bind(lease_id)
        .bind(lease_seconds)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn release(pool: &PgPool, lease_id: Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM execution_log_stream_lease WHERE id = $1")
            .bind(lease_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn active_count(pool: &PgPool) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution_log_stream_lease WHERE expires_at > clock_timestamp()",
        )
        .fetch_one(pool)
        .await
        .map_err(Into::into)
    }
}
