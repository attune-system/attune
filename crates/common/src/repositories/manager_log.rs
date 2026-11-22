//! Server-side log key derivation and stable slot binding.
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use sqlx::{Executor, PgConnection, Postgres};

use crate::{models::Id, Error, Result};

use super::manager_session::ManagerSessionRecord;

#[derive(sqlx::FromRow)]
pub struct ManagerLogScope {
    pub version: Id,
    pub assignment: Id,
    pub worker: Id,
    pub pool: String,
    pub slot: String,
}

pub struct ManagerLogRepository;

fn denied() -> Error {
    Error::PermissionDenied("Manager log is unavailable".into())
}

impl ManagerLogRepository {
    pub async fn bind(
        conn: &mut PgConnection,
        assignment: Id,
        version: Id,
        worker: Id,
        manager: &ManagerSessionRecord,
        stream: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO manager_log_scope(version,assignment,worker,pool,slot,stream_name)
             VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(version)
        .bind(assignment)
        .bind(worker)
        .bind(&manager.pool)
        .bind(&manager.name)
        .bind(stream)
        .execute(conn)
        .await?;
        Ok(())
    }

    pub async fn find<'e>(
        executor: impl Executor<'e, Database = Postgres>,
        version: Id,
    ) -> Result<Option<ManagerLogScope>> {
        Ok(sqlx::query_as(
            "SELECT version,assignment,worker,pool,slot FROM manager_log_scope WHERE version=$1",
        )
        .bind(version)
        .fetch_optional(executor)
        .await?)
    }

    pub async fn existing(
        conn: &mut PgConnection,
        assignment: Id,
        stream: &str,
    ) -> Result<Option<Id>> {
        Ok(sqlx::query_scalar(
            "SELECT version FROM manager_log_scope WHERE assignment=$1 AND stream_name=$2",
        )
        .bind(assignment)
        .bind(stream)
        .fetch_optional(conn)
        .await?)
    }

    /// Retain the currently enrolled process of the bound stable slot, which
    /// may be distinct from the process that captured the bytes.
    pub async fn retain_slot(
        conn: &mut PgConnection,
        version: Id,
        worker: Id,
        instance: uuid::Uuid,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let Some(scope) = Self::find(&mut *conn, version).await? else {
            return Ok(None);
        };
        if scope.worker != worker {
            return Err(denied());
        }
        let deadline = sqlx::query_scalar(
            "SELECT s.expires_at FROM manager_session s
             JOIN manager_action_worker m ON m.manager_session=s.id
             JOIN worker w ON w.id=m.worker
             WHERE s.role='action' AND s.pool=$1 AND s.name=$2 AND s.instance=$3
               AND s.closed_at IS NULL AND s.expires_at>clock_timestamp() AND m.worker=$4
               AND w.meta->>'registration_incarnation'=s.instance::text
             FOR SHARE OF s",
        )
        .bind(&scope.pool)
        .bind(&scope.slot)
        .bind(instance)
        .bind(worker)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        Ok(Some(deadline))
    }

    /// Streaming captures share the execution and the current manager session.
    /// Finish and reaping already acquire the assignment before the execution,
    /// so this effect never takes the assignment lock in the opposite order.
    pub async fn require_capture(
        conn: &mut PgConnection,
        version: Id,
        credential: &crate::log_delivery::CaptureCredential,
    ) -> Result<chrono::DateTime<chrono::Utc>> {
        if credential.version != version {
            return Err(denied());
        }
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_capture(conn).await?;
        let scope = Self::find(&mut *conn, version).await?.ok_or_else(denied)?;
        if scope.worker != credential.worker {
            return Err(denied());
        }
        let (execution, action, pack): (Id, Id, Id) = sqlx::query_as(
            "SELECT e.id, e.action, a.pack
             FROM manager_execution_assignment m
             JOIN execution e ON e.id=m.execution
             JOIN action a ON a.id=e.action
             JOIN artifact_version v ON v.execution=e.id AND v.id=$1
             WHERE m.id=$2 AND e.worker=$3 AND e.status='running'
               AND e.pack_release=m.pack_release AND e.pack_release_digest=m.pack_release_digest
             FOR SHARE OF e",
        )
        .bind(version)
        .bind(scope.assignment)
        .bind(scope.worker)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        super::delegated_execution::DelegatedExecutionRepository::require_completion(
            conn,
            execution,
            scope.worker,
        )
        .await?;
        let artifact = sqlx::query_scalar("SELECT artifact FROM artifact_version WHERE id=$1")
            .bind(version)
            .fetch_one(&mut *conn)
            .await?;
        super::worker_delivery::WorkerDeliveryRepository::require_log_owner(
            conn, artifact, action, pack,
        )
        .await?;
        let session_deadline = Self::retain_slot(conn, version, scope.worker, credential.instance)
            .await?
            .ok_or_else(denied)?;
        let deadline: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT deadline FROM manager_execution_assignment WHERE id=$1 AND manager_session=$2
                 AND instance=$3 AND worker=$4 AND closed_at IS NULL AND claim_request IS NOT NULL
                 AND deadline>clock_timestamp()",
        )
        .bind(scope.assignment)
        .bind(credential.session)
        .bind(credential.instance)
        .bind(scope.worker)
        .fetch_optional(conn)
        .await?
        .ok_or_else(denied)?;
        Ok(deadline.min(session_deadline))
    }

    pub async fn key<'e>(
        executor: impl Executor<'e, Database = Postgres>,
        version: Id,
        platform_key: Option<&str>,
    ) -> Result<Option<String>> {
        let Some(scope) = Self::find(executor, version).await? else {
            return Ok(platform_key.map(str::to_owned));
        };
        Ok(Some(scope.derive_key(platform_key)?))
    }
}

impl ManagerLogScope {
    pub fn derive_key(&self, platform_key: Option<&str>) -> Result<String> {
        let key = platform_key
            .filter(|key| !key.is_empty())
            .ok_or_else(denied)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(|_| denied())?;
        mac.update(&serde_json::to_vec(&(
            "attune/manager-log-stream/v1",
            self.version,
            self.assignment,
            self.worker,
            &self.pool,
            &self.slot,
        ))?);
        Ok(mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
}
