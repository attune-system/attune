//! Pack Install Repository
//!
//! Database operations for pack installation tracking. Records survive a
//! rolled-back new-pack install so the failure (and test snapshot) can still
//! be queried by pack ref.

use crate::error::Result;
use crate::models::{Id, PackInstall, PackInstallStatus};
use sqlx::PgPool;

/// Repository for pack install lifecycle records
pub struct PackInstallRepository {
    pool: PgPool,
}

const PACK_INSTALL_COLUMNS: &str = "id, pack_ref, pack_version, status, trigger_reason, \
    pack_id, test_execution_id, test_result, error_message, started_at, updated_at, finished_at";

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
    ) -> Result<PackInstall> {
        let sql = format!(
            "INSERT INTO pack_install (pack_ref, pack_version, status, trigger_reason, pack_id) \
             VALUES ($1, $2, $3, $4, $5) RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        let record = sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(pack_ref)
            .bind(pack_version)
            .bind(PackInstallStatus::Pending.as_str())
            .bind(trigger_reason)
            .bind(pack_id)
            .fetch_one(&self.pool)
            .await?;

        Ok(record)
    }

    /// Find an install record by ID
    pub async fn find_by_id(&self, id: Id) -> Result<Option<PackInstall>> {
        let sql = format!("SELECT {} FROM pack_install WHERE id = $1", PACK_INSTALL_COLUMNS);
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

    /// Transition the record into a running state (called when a test has been dispatched).
    pub async fn mark_running(&self, id: Id) -> Result<PackInstall> {
        self.update_status(id, PackInstallStatus::Running, None).await
    }

    /// Record a terminal state with optional result/error detail.
    pub async fn finish(
        &self,
        id: Id,
        status: PackInstallStatus,
        test_execution_id: Option<Id>,
        test_result: Option<serde_json::Value>,
        error_message: Option<String>,
    ) -> Result<PackInstall> {
        let sql = format!(
            "UPDATE pack_install \
             SET status = $2, \
                 test_execution_id = COALESCE($3, test_execution_id), \
                 test_result = COALESCE($4, test_result), \
                 error_message = $5, \
                 finished_at = COALESCE(finished_at, NOW()), \
                 updated_at = NOW() \
             WHERE id = $1 RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        let record = sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(status.as_str())
            .bind(test_execution_id)
            .bind(test_result)
            .bind(error_message)
            .fetch_one(&self.pool)
            .await?;

        Ok(record)
    }

    /// Update status with an optional error message.
    pub async fn update_status(
        &self,
        id: Id,
        status: PackInstallStatus,
        error_message: Option<String>,
    ) -> Result<PackInstall> {
        let sql = format!(
            "UPDATE pack_install SET status = $2, error_message = COALESCE($3, error_message), \
             updated_at = NOW() WHERE id = $1 RETURNING {}",
            PACK_INSTALL_COLUMNS
        );
        let record = sqlx::query_as::<_, PackInstall>(sql.as_str())
            .bind(id)
            .bind(status.as_str())
            .bind(error_message)
            .fetch_one(&self.pool)
            .await?;

        Ok(record)
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