use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::manager_control::{ManagerDatabaseCredential, ManagerDatabaseCredentialRequest};

#[derive(sqlx::FromRow)]
pub struct ManagerDatabasePodCheck {
    pub session_id: i64,
    pub pod_uid: Uuid,
    pub pod_name: String,
    pub manager_role: crate::models::WorkerRole,
    pub pool: String,
}

/// Retire expired or closed logins managed by the supervisor.
pub struct ManagerDatabaseLoginRepository;

impl ManagerDatabaseLoginRepository {
    pub async fn claim_pod_checks(
        pool: &PgPool,
        limit: i32,
    ) -> crate::Result<Vec<ManagerDatabasePodCheck>> {
        Ok(sqlx::query_as::<_, ManagerDatabasePodCheck>("SELECT session_id, pod_uid, pod_name, manager_role, pool FROM claim_manager_db_pod_checks($1)")
            .bind(limit).fetch_all(pool).await?)
    }

    pub async fn invalidate_pod(pool: &PgPool, session: i64, pod_uid: Uuid) -> crate::Result<()> {
        sqlx::query("SELECT invalidate_manager_db_pod($1, $2)")
            .bind(session)
            .bind(pod_uid)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn inspect(
        db: &mut PgConnection,
        role: crate::models::WorkerRole,
        instance: Uuid,
        session: i64,
    ) -> crate::Result<bool> {
        let query = match role {
            crate::models::WorkerRole::Action => {
                "SELECT session_id FROM manager_db_action_session($1) WHERE closed_at IS NULL"
            }
            crate::models::WorkerRole::Sensor => {
                "SELECT session_id FROM manager_db_sensor_session($1) WHERE closed_at IS NULL"
            }
        };
        Ok(sqlx::query_scalar::<_, i64>(query)
            .bind(instance)
            .fetch_optional(db)
            .await?
            == Some(session))
    }

    pub async fn request(
        tx: &mut Transaction<'_, Postgres>,
        session: i64,
        pod_uid: Uuid,
        request: &ManagerDatabaseCredentialRequest,
    ) -> crate::Result<bool> {
        let result = sqlx::query(
            r#"INSERT INTO manager_db_credential
                (manager_session, pod_uid, request_id, requested_generation,
                 proof_expires_at, pending_verifier)
            SELECT $1, $2, $3, $4, clock_timestamp() + interval '2 minutes', $5
            WHERE $4 = 1 OR EXISTS (
                SELECT 1 FROM manager_db_credential WHERE manager_session = $1
            )
            ON CONFLICT (manager_session) DO UPDATE
            SET requested_generation = EXCLUDED.requested_generation,
                request_id = EXCLUDED.request_id,
                pending_verifier = CASE WHEN manager_db_credential.installed_generation = $4
                    THEN NULL ELSE EXCLUDED.pending_verifier END,
                proof_expires_at = CASE WHEN manager_db_credential.installed_generation = $4
                    THEN manager_db_credential.proof_expires_at ELSE EXCLUDED.proof_expires_at END
            WHERE manager_db_credential.pod_uid = $2 AND manager_db_credential.retired_at IS NULL
                AND NOT manager_db_credential.pod_invalidated
                AND (
                    (manager_db_credential.request_id = $3
                      AND manager_db_credential.requested_generation = $4
                      AND (manager_db_credential.pending_verifier IS NULL
                           OR manager_db_credential.pending_verifier = $5))
                    OR
                    (manager_db_credential.requested_generation = manager_db_credential.installed_generation
                      AND manager_db_credential.installed_generation = $4 - 1)
                )"#,
        )
        .bind(session)
        .bind(pod_uid)
        .bind(request.request)
        .bind(request.generation)
        .bind(&request.verifier)
        .execute(&mut **tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn current(
        tx: &mut Transaction<'_, Postgres>,
        session: i64,
    ) -> crate::Result<Option<ManagerDatabaseCredential>> {
        Ok(sqlx::query_as::<_, ManagerDatabaseCredential>(
            r#"SELECT request_id AS request, installed_generation AS generation,
                      login_name::TEXT AS username, credential_expires_at AS expires_at,
                      greatest(2, extract(epoch FROM credential_expires_at
                          - clock_timestamp())::BIGINT - 60) AS rotate_after_seconds
               FROM manager_db_credential
               WHERE manager_session = $1 AND retired_at IS NULL AND NOT pod_invalidated
                 AND installed_generation = requested_generation
                 AND credential_expires_at > clock_timestamp()"#,
        )
        .bind(session)
        .fetch_optional(&mut **tx)
        .await?)
    }

    pub async fn install_requested(pool: &PgPool, batch_size: i32) -> crate::Result<i32> {
        Ok(
            sqlx::query_scalar("SELECT install_requested_manager_db_logins($1)")
                .bind(batch_size)
                .fetch_one(pool)
                .await?,
        )
    }

    pub async fn disable_retired(pool: &PgPool, batch_size: i32) -> crate::Result<i32> {
        Ok(
            sqlx::query_scalar("SELECT disable_retired_manager_db_logins($1)")
                .bind(batch_size)
                .fetch_one(pool)
                .await?,
        )
    }

    /// Call after the disabling operation commits so a new connection cannot
    /// authenticate while the existing ones are being drained.
    pub async fn drain_retired(pool: &PgPool, batch_size: i32) -> crate::Result<i32> {
        Ok(
            sqlx::query_scalar("SELECT drain_retired_manager_db_connections($1)")
                .bind(batch_size)
                .fetch_one(pool)
                .await?,
        )
    }
}
