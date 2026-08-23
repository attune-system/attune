//! Pack Install Repository
//!
//! Database operations for pack installation tracking. Records survive a
//! rolled-back new-pack install so the failure (and test snapshot) can still
//! be queried by pack ref.

use crate::error::Result;
use crate::models::{Id, PackInstall, PackInstallStatus};
use sqlx::{PgConnection, PgPool};

/// Repository for pack install lifecycle records
pub struct PackInstallRepository {
    pool: PgPool,
}

const PACK_INSTALL_COLUMNS: &str = "id, pack_ref, pack_version, status, trigger_reason, \
    pack_id, requested_by, assigned_worker_id, candidate_access_token_hash, test_execution_id, \
    test_result, error_message, started_at, updated_at, finished_at";

/// Hard upper bound for an unfinished pack-install attempt.
pub const PACK_INSTALL_ACTIVE_TTL_SECS: i64 = 7 * 60 * 60;
pub const PACK_INSTALL_ACTIVATION_TTL_SECS: i64 = 15 * 60;

impl PackInstallRepository {
    /// Create a new pack install repository
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a new install record
    pub async fn create(
        &self,
        pack_ref: &str,
        pack_version: &str,
        trigger_reason: &str,
        pack_id: Option<Id>,
        requested_by: Option<Id>,
    ) -> Result<PackInstall> {
        let sql = format!(
            "INSERT INTO pack_install (pack_ref, pack_version, status, trigger_reason, pack_id, requested_by) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        let record = sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(pack_ref)
            .bind(pack_version)
            .bind(PackInstallStatus::Pending.as_str())
            .bind(trigger_reason)
            .bind(pack_id)
            .bind(requested_by)
            .fetch_one(&self.pool)
            .await?;

        Ok(record)
    }

    /// Find an install record by ID
    pub async fn find_by_id(&self, id: Id) -> Result<Option<PackInstall>> {
        let sql = format!(
            "SELECT {} FROM pack_install WHERE id = $1",
            PACK_INSTALL_COLUMNS
        );
        let record = sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(record)
    }

    /// Find the most recent install record for a pack ref
    pub async fn find_latest_by_pack_ref(&self, pack_ref: &str) -> Result<Option<PackInstall>> {
        let sql = format!(
            "SELECT {} FROM pack_install WHERE pack_ref = $1 ORDER BY id DESC LIMIT 1",
            PACK_INSTALL_COLUMNS
        );
        let record = sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(pack_ref)
            .fetch_optional(&self.pool)
            .await?;

        Ok(record)
    }

    /// Claim a pending install for one worker and transition it to running.
    pub async fn claim_worker(
        &self,
        id: Id,
        worker_id: Id,
        candidate_access_token_hash: Option<&str>,
    ) -> Result<Option<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install \
             SET status = $2, assigned_worker_id = $3, candidate_access_token_hash = $4, \
                 updated_at = NOW() \
             WHERE id = $1 AND status = 'pending' RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(PackInstallStatus::Running.as_str())
            .bind(worker_id)
            .bind(candidate_access_token_hash)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Record a terminal state only while dispatch or execution is active.
    pub async fn finish_active(
        &self,
        id: Id,
        status: PackInstallStatus,
        test_execution_id: Option<Id>,
        test_result: Option<serde_json::Value>,
        error_message: Option<String>,
    ) -> Result<Option<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install \
             SET status = $2, \
                 test_execution_id = COALESCE($3, test_execution_id), \
                 test_result = COALESCE($4, test_result), \
                 error_message = $5, \
                 finished_at = COALESCE(finished_at, NOW()), \
                 updated_at = NOW() \
             WHERE id = $1 AND status IN ('pending', 'running') RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(status.as_str())
            .bind(test_execution_id)
            .bind(test_result)
            .bind(error_message)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Record a dispatch failure only before an executor claims the attempt.
    pub async fn finish_pending(
        &self,
        id: Id,
        error_message: String,
    ) -> Result<Option<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install \
             SET status = $2, error_message = $3, finished_at = COALESCE(finished_at, NOW()), \
                 updated_at = NOW() \
             WHERE id = $1 AND status = 'pending' RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(PackInstallStatus::Failed.as_str())
            .bind(error_message)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Record a worker result only while the assigned test is still running.
    pub async fn finish_running(
        &self,
        id: Id,
        status: PackInstallStatus,
        test_execution_id: Option<Id>,
        test_result: Option<serde_json::Value>,
        error_message: Option<String>,
    ) -> Result<Option<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install \
             SET status = $2, \
                 test_execution_id = COALESCE($3, test_execution_id), \
                 test_result = COALESCE($4, test_result), \
                 error_message = $5, \
                 candidate_access_token_hash = NULL, \
                 finished_at = CASE WHEN $2 = 'activating' THEN NULL ELSE COALESCE(finished_at, NOW()) END, \
                 updated_at = NOW() \
             WHERE id = $1 AND status = 'running' RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(status.as_str())
            .bind(test_execution_id)
            .bind(test_result)
            .bind(error_message)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Finish API-side activation after a worker has supplied a passing result.
    pub async fn finish_activation(
        &self,
        id: Id,
        status: PackInstallStatus,
        error_message: Option<String>,
    ) -> Result<Option<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install \
             SET status = $2, error_message = $3, finished_at = NOW(), updated_at = NOW() \
             WHERE id = $1 AND status = 'activating' RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(status.as_str())
            .bind(error_message)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Finish activation in the same transaction that commits pack metadata.
    pub async fn finish_activation_in_transaction(
        connection: &mut PgConnection,
        id: Id,
        pack_id: Id,
    ) -> Result<Option<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install \
             SET status = 'succeeded', pack_id = $2, error_message = NULL, \
                 finished_at = NOW(), updated_at = NOW() \
             WHERE id = $1 AND status = 'activating' RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(pack_id)
            .fetch_optional(connection)
            .await?)
    }

    /// Fail unfinished attempts that exceeded the hard lifecycle deadline.
    pub async fn fail_stale_active(&self) -> Result<Vec<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install \
             SET status = 'failed', candidate_access_token_hash = NULL, \
                 error_message = 'Pack install exceeded the maximum active lifetime', \
                 finished_at = NOW(), updated_at = NOW() \
             WHERE (status IN ('pending', 'running') \
                    AND started_at < NOW() - make_interval(secs => $1)) \
                OR (status = 'activating' \
                    AND updated_at < NOW() - make_interval(secs => $2)) \
             RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(PACK_INSTALL_ACTIVE_TTL_SECS as f64)
            .bind(PACK_INSTALL_ACTIVATION_TTL_SECS as f64)
            .fetch_all(&self.pool)
            .await?)
    }

    /// Bind a completed pre-activation attempt to the pack and test-history row it produced.
    pub async fn attach_pack_result(
        &self,
        id: Id,
        pack_id: Id,
        test_execution_id: Option<Id>,
    ) -> Result<Option<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install \
             SET pack_id = $2, test_execution_id = COALESCE($3, test_execution_id), \
                 updated_at = NOW() \
             WHERE id = $1 AND status IN ('succeeded', 'failed') \
               AND (pack_id IS NULL OR pack_id = $2) RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(pack_id)
            .bind(test_execution_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Mark a failed new-pack install as rolled back.
    pub async fn mark_rolled_back(&self, id: Id) -> Result<Option<PackInstall>> {
        let sql = format!(
            "UPDATE pack_install SET status = $2, updated_at = NOW() \
             WHERE id = $1 AND status = 'failed' RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        Ok(sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(PackInstallStatus::RolledBack.as_str())
            .fetch_optional(&self.pool)
            .await?)
    }

    /// List recent installs for a pack ref (newest first).
    pub async fn list_by_pack_ref(
        &self,
        pack_ref: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PackInstall>> {
        let sql = format!(
            "SELECT {} FROM pack_install WHERE pack_ref = $1 ORDER BY id DESC LIMIT $2 OFFSET $3",
            PACK_INSTALL_COLUMNS
        );
        let records = sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(pack_ref)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

        Ok(records)
    }
}

/// Whether a persisted pack_install status string is terminal.
pub fn pack_install_is_terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "rolled_back")
}
