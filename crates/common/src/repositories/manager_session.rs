use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::manager_control::{ManagerEnrollment, SESSION_TTL_SECONDS};
use crate::models::{Id, WorkerRole};
use crate::Result;

const COLUMNS: &str = "id, role, pool, name, instance, enrollment_digest, expires_at, closed_at";

#[derive(FromRow)]
pub struct ManagerSessionRecord {
    pub id: Id,
    pub role: WorkerRole,
    pub pool: String,
    pub name: String,
    pub instance: Uuid,
    pub enrollment_digest: String,
    pub expires_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

pub struct ManagerSessionRepository;

impl ManagerSessionRepository {
    pub async fn enroll(
        pool: &PgPool,
        enrollment: &ManagerEnrollment,
        digest: &str,
    ) -> Result<Option<ManagerSessionRecord>> {
        let mut tx = pool.begin().await?;
        // Serialize vacant-slot enrollment as well as registration replacement.
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtext(current_schema() || ':manager_session'),
             hashtext($1::worker_role_enum::text || ':' || $2 || ':' || $3))",
        )
        .bind(enrollment.role)
        .bind(&enrollment.pool)
        .bind(&enrollment.name)
        .execute(&mut *tx)
        .await?;

        // Replays recover the same live session without extending its lifetime.
        let previous: Option<(Id, bool)> = sqlx::query_as(
            "SELECT id, closed_at IS NULL AND expires_at > clock_timestamp() AND enrollment_digest = $5
             FROM manager_session WHERE role = $1 AND pool = $2 AND name = $3 AND instance = $4 FOR UPDATE",
        )
        .bind(enrollment.role)
        .bind(&enrollment.pool)
        .bind(&enrollment.name)
        .bind(enrollment.instance)
        .bind(digest)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, current)) = previous {
            if !current {
                return Ok(None);
            }
            let record = sqlx::query_as(&format!(
                "SELECT {COLUMNS} FROM manager_session WHERE id = $1"
            ))
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(Some(record));
        }

        sqlx::query(
            "UPDATE manager_session SET closed_at = clock_timestamp()
             WHERE role = $1 AND pool = $2 AND name = $3 AND closed_at IS NULL
             AND expires_at <= clock_timestamp()",
        )
        .bind(enrollment.role)
        .bind(&enrollment.pool)
        .bind(&enrollment.name)
        .execute(&mut *tx)
        .await?;

        let record = sqlx::query_as(&format!(
            "INSERT INTO manager_session (role, pool, name, instance, enrollment_digest, expires_at)
             VALUES ($1, $2, $3, $4, $5, date_trunc('second', clock_timestamp()) + make_interval(secs => $6::integer))
             ON CONFLICT (role, pool, name) WHERE closed_at IS NULL DO NOTHING RETURNING {COLUMNS}",
        ))
        .bind(enrollment.role)
        .bind(&enrollment.pool)
        .bind(&enrollment.name)
        .bind(enrollment.instance)
        .bind(digest)
        .bind(SESSION_TTL_SECONDS)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Hold the session fence until the caller commits the authorized mutation.
    /// Only withdrawal retries can use a closed session.
    pub async fn lock_for_update(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Id,
        instance: Uuid,
        token_expiry: DateTime<Utc>,
        allow_closed: bool,
    ) -> Result<Option<ManagerSessionRecord>> {
        Self::lock(
            tx,
            id,
            instance,
            Some(token_expiry),
            allow_closed,
            "FOR UPDATE",
        )
        .await
    }

    /// Concurrent work operations share an active session until their effect commits.
    pub async fn lock_for_work(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Id,
        instance: Uuid,
        token_expiry: DateTime<Utc>,
    ) -> Result<Option<ManagerSessionRecord>> {
        Self::lock(tx, id, instance, Some(token_expiry), false, "FOR SHARE").await
    }

    /// An already authorized streaming request retains the exact session ID
    /// through I/O. The current session is still required at publication.
    pub async fn lock_for_continuation(
        tx: &mut Transaction<'_, Postgres>,
        id: Id,
        instance: Uuid,
    ) -> Result<Option<ManagerSessionRecord>> {
        Self::lock(tx, id, instance, None, false, "FOR SHARE").await
    }

    async fn lock(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Id,
        instance: Uuid,
        token_expiry: Option<DateTime<Utc>>,
        allow_closed: bool,
        sql_lock: &'static str,
    ) -> Result<Option<ManagerSessionRecord>> {
        Ok(sqlx::query_as(&format!(
            "WITH locked AS MATERIALIZED (
                 SELECT {COLUMNS} FROM manager_session WHERE id = $1 AND instance = $2 {sql_lock}
             )
             SELECT {COLUMNS} FROM locked
             WHERE (closed_at IS NULL OR $4) AND LEAST(expires_at, $3::timestamptz) > clock_timestamp()",
        ))
        .bind(id)
        .bind(instance)
        .bind(token_expiry)
        .bind(allow_closed)
        .fetch_optional(&mut **tx)
        .await?)
    }

    pub async fn renew(
        conn: &mut PgConnection,
        id: Id,
        token_expiry: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>> {
        Ok(sqlx::query_scalar(
            "UPDATE manager_session SET expires_at = date_trunc('second', clock_timestamp()) + make_interval(secs => $2::integer)
             WHERE id = $1 AND closed_at IS NULL
             AND LEAST(expires_at, $3) > clock_timestamp() RETURNING expires_at",
        )
        .bind(id)
        .bind(SESSION_TTL_SECONDS)
        .bind(token_expiry)
        .fetch_optional(conn)
        .await?)
    }

    pub async fn withdraw(
        conn: &mut PgConnection,
        id: Id,
        token_expiry: DateTime<Utc>,
    ) -> Result<bool> {
        let updated = sqlx::query(
            "UPDATE manager_session SET closed_at = clock_timestamp()
             WHERE id = $1 AND closed_at IS NULL
             AND LEAST(expires_at, $2) > clock_timestamp()",
        )
        .bind(id)
        .bind(token_expiry)
        .execute(conn)
        .await?;
        Ok(updated.rows_affected() == 1)
    }
}
