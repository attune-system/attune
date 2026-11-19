//! Pack Environment Manager
//!
//! Tracks per-pack runtime metadata and coordinated environment publication.
//! The owning worker supplies process effects; this repository helper never
//! executes pack-declared installer commands.

use crate::error::{Error, Result};
use crate::models::{Runtime, RuntimeVersion};
use crate::repositories::action::ActionRepository;
use crate::repositories::runtime::{self, RuntimeRepository};
use crate::repositories::{FindById as _, FindByRef as _};
use crate::runtime_detection::normalize_runtime_name;
use serde_json::Value as JsonValue;
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction, Type};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Status of a pack environment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type)]
#[sqlx(type_name = "pack_environment_status_enum", rename_all = "lowercase")]
pub enum PackEnvironmentStatus {
    Pending,
    Installing,
    Ready,
    Failed,
    Outdated,
}

impl PackEnvironmentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Installing => "installing",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Outdated => "outdated",
        }
    }

    pub fn parse_status(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "installing" => Some(Self::Installing),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            "outdated" => Some(Self::Outdated),
            _ => None,
        }
    }
}

/// Pack environment record
#[derive(Debug, Clone)]
pub struct PackEnvironment {
    pub id: i64,
    pub pack: i64,
    pub pack_ref: String,
    pub runtime: i64,
    pub runtime_ref: String,
    pub env_path: String,
    pub status: PackEnvironmentStatus,
    pub installed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_verified: Option<chrono::DateTime<chrono::Utc>>,
    pub install_log: Option<String>,
    pub install_error: Option<String>,
    pub metadata: JsonValue,
}

/// Version-aware coordination record for a shared pack runtime environment.
#[derive(Debug, Clone)]
pub struct CoordinatedPackEnvironment {
    pub id: i64,
    pub pack: i64,
    pub pack_ref: String,
    pub runtime: i64,
    pub runtime_ref: String,
    pub runtime_version: Option<i64>,
    pub runtime_version_text: Option<String>,
    pub env_key: String,
    pub env_path: String,
    pub status: PackEnvironmentStatus,
    pub manifest_checksum: Option<String>,
    pub claimed_by_worker: Option<i64>,
    pub claim_id: Option<Uuid>,
    pub claim_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub installed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_verified: Option<chrono::DateTime<chrono::Utc>>,
    pub install_log: Option<String>,
    pub install_error: Option<String>,
    pub metadata: JsonValue,
}

#[derive(Debug, Clone)]
pub struct UpsertCoordinatedEnvironmentInput<'a> {
    pub pack_id: i64,
    pub pack_ref: &'a str,
    pub runtime_id: i64,
    pub runtime_ref: &'a str,
    pub runtime_version: Option<&'a RuntimeVersion>,
    pub env_path: &'a Path,
    pub manifest_checksum: Option<&'a str>,
}

/// Pack environment manager
#[derive(Clone)]
pub struct PackEnvironmentManager {
    pool: PgPool,
}

impl PackEnvironmentManager {
    /// Track environment metadata; process effects belong to the owning worker.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn pack_environment_from_row(row: &PgRow) -> Result<PackEnvironment> {
        Ok(PackEnvironment {
            id: row.try_get("id")?,
            pack: row.try_get("pack")?,
            pack_ref: row.try_get("pack_ref")?,
            runtime: row.try_get("runtime")?,
            runtime_ref: row.try_get("runtime_ref")?,
            env_path: row.try_get("env_path")?,
            status: row.try_get("status")?,
            installed_at: row.try_get("installed_at")?,
            last_verified: row.try_get("last_verified")?,
            install_log: row.try_get("install_log")?,
            install_error: row.try_get("install_error")?,
            metadata: row.try_get("metadata")?,
        })
    }

    fn coordinated_environment_from_row(row: &PgRow) -> Result<CoordinatedPackEnvironment> {
        Ok(CoordinatedPackEnvironment {
            id: row.try_get("id")?,
            pack: row.try_get("pack")?,
            pack_ref: row.try_get("pack_ref")?,
            runtime: row.try_get("runtime")?,
            runtime_ref: row.try_get("runtime_ref")?,
            runtime_version: row.try_get("runtime_version")?,
            runtime_version_text: row.try_get("runtime_version_text")?,
            env_key: row.try_get("env_key")?,
            env_path: row.try_get("env_path")?,
            status: row.try_get("status")?,
            manifest_checksum: row.try_get("manifest_checksum")?,
            claimed_by_worker: row.try_get("claimed_by_worker")?,
            claim_id: row.try_get("claim_id")?,
            claim_expires_at: row.try_get("claim_expires_at")?,
            installed_at: row.try_get("installed_at")?,
            last_verified: row.try_get("last_verified")?,
            install_log: row.try_get("install_log")?,
            install_error: row.try_get("install_error")?,
            metadata: row.try_get("metadata")?,
        })
    }

    /// Get an existing pack environment
    pub async fn get_environment(
        &self,
        pack_id: i64,
        runtime_id: i64,
    ) -> Result<Option<PackEnvironment>> {
        let row = sqlx::query(
            r#"
            SELECT id, pack, pack_ref, runtime, runtime_ref, env_path, status,
                   installed_at, last_verified, install_log, install_error, metadata
            FROM pack_environment
            WHERE pack = $1 AND runtime = $2 AND runtime_version IS NULL
            "#,
        )
        .bind(pack_id)
        .bind(runtime_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            Ok(Some(Self::pack_environment_from_row(&row)?))
        } else {
            Ok(None)
        }
    }

    /// Get the executable path for a pack environment
    pub async fn get_executable_path(
        &self,
        pack_id: i64,
        runtime_id: i64,
        executable_name: &str,
    ) -> Result<Option<String>> {
        let env = match self.get_environment(pack_id, runtime_id).await? {
            Some(e) => e,
            None => return Ok(None),
        };

        if env.status != PackEnvironmentStatus::Ready {
            return Ok(None);
        }

        // Get runtime to check executable templates
        let runtime = self.get_runtime(runtime_id).await?;

        let executable_path =
            if let Some(templates) = runtime.installers.get("executable_templates") {
                if let Some(template) = templates.get(executable_name) {
                    if let Some(template_str) = template.as_str() {
                        self.resolve_template(
                            template_str,
                            &env.pack_ref,
                            &env.runtime_ref,
                            &env.env_path,
                            "",
                        )?
                    } else {
                        return Ok(None);
                    }
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            };

        Ok(Some(executable_path))
    }

    /// Delete a pack environment
    pub async fn delete_environment(&self, pack_id: i64, runtime_id: i64) -> Result<()> {
        let env = match self.get_environment(pack_id, runtime_id).await? {
            Some(e) => e,
            None => {
                debug!(
                    "No environment to delete for pack {} runtime {}",
                    pack_id, runtime_id
                );
                return Ok(());
            }
        };

        info!("Deleting environment: {}", env.env_path);

        // Delete filesystem directory
        let env_path = PathBuf::from(&env.env_path);
        if env_path.exists() {
            fs::remove_dir_all(&env_path).await.map_err(|e| {
                Error::Internal(format!("Failed to delete environment directory: {}", e))
            })?;
            info!("Deleted environment directory: {}", env.env_path);
        }

        // Delete database record
        sqlx::query("DELETE FROM pack_environment WHERE id = $1")
            .bind(env.id)
            .execute(&self.pool)
            .await?;

        info!(
            "Deleted environment record for pack {} runtime {}",
            pack_id, runtime_id
        );

        Ok(())
    }

    /// Verify an environment is still functional
    pub async fn verify_environment(&self, pack_id: i64, runtime_id: i64) -> Result<bool> {
        let env = match self.get_environment(pack_id, runtime_id).await? {
            Some(e) => e,
            None => return Ok(false),
        };

        if env.status != PackEnvironmentStatus::Ready {
            return Ok(false);
        }

        // Check if directory exists
        let env_path = PathBuf::from(&env.env_path);
        if !env_path.exists() {
            warn!("Environment path does not exist: {}", env.env_path);
            self.mark_environment_outdated(env.id).await?;
            return Ok(false);
        }

        // Update last_verified timestamp
        sqlx::query("UPDATE pack_environment SET last_verified = NOW() WHERE id = $1")
            .bind(env.id)
            .execute(&self.pool)
            .await?;

        Ok(true)
    }

    /// List all environments for a pack
    pub async fn list_pack_environments(&self, pack_id: i64) -> Result<Vec<PackEnvironment>> {
        let rows = sqlx::query(
            r#"
            SELECT id, pack, pack_ref, runtime, runtime_ref, env_path, status,
                   installed_at, last_verified, install_log, install_error, metadata
            FROM pack_environment
            WHERE pack = $1
            AND runtime_version IS NULL
            ORDER BY runtime_ref
            "#,
        )
        .bind(pack_id)
        .fetch_all(&self.pool)
        .await?;

        let mut environments = Vec::new();
        for row in rows {
            environments.push(Self::pack_environment_from_row(&row)?);
        }

        Ok(environments)
    }

    pub async fn list_pack_environment_paths_in_transaction(
        tx: &mut Transaction<'_, Postgres>,
        pack_id: i64,
    ) -> Result<Vec<PathBuf>> {
        let paths = sqlx::query_scalar::<_, String>(
            r#"
            SELECT DISTINCT owned.env_path
            FROM pack_environment owned
            WHERE owned.pack = $1
              AND NOT EXISTS (
                  SELECT 1
                  FROM pack_environment shared
                  WHERE shared.pack != $1
                    AND shared.env_path = owned.env_path
              )
            ORDER BY owned.env_path
            "#,
        )
        .bind(pack_id)
        .fetch_all(&mut **tx)
        .await?;

        Ok(paths.into_iter().map(PathBuf::from).collect())
    }

    // ========================================================================
    // Private helper methods
    // ========================================================================

    async fn get_runtime(&self, runtime_id: i64) -> Result<Runtime> {
        let query = format!(
            "SELECT {} FROM runtime WHERE id = $1",
            runtime::SELECT_COLUMNS
        );
        sqlx::query_as::<_, Runtime>(&query)
            .bind(runtime_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch runtime: {}", e)))
    }

    fn resolve_template(
        &self,
        template: &str,
        pack_ref: &str,
        runtime_ref: &str,
        env_path: &str,
        pack_path: &str,
    ) -> Result<String> {
        let result = template
            .replace("{env_path}", env_path)
            .replace("{pack_path}", pack_path)
            .replace("{pack_ref}", pack_ref)
            .replace("{runtime_ref}", runtime_ref);

        Ok(result)
    }

    async fn mark_environment_outdated(&self, env_id: i64) -> Result<()> {
        sqlx::query("UPDATE pack_environment SET status = 'outdated',claimed_by_worker=NULL,claim_id=NULL,claim_expires_at=NULL WHERE id = $1")
            .bind(env_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn upsert_coordinated_environment(
        &self,
        input: UpsertCoordinatedEnvironmentInput<'_>,
    ) -> Result<CoordinatedPackEnvironment> {
        let env_path_str = input.env_path.to_string_lossy().to_string();
        let runtime_version_id = input.runtime_version.map(|version| version.id);
        let runtime_version_text = input
            .runtime_version
            .map(|version| version.version.as_str());

        let row = sqlx::query(
            r#"
            INSERT INTO pack_environment (
                pack, pack_ref, runtime, runtime_ref, runtime_version, runtime_version_text,
                env_path, status, manifest_checksum
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', $8)
            ON CONFLICT (env_key)
            DO UPDATE SET
                pack_ref = EXCLUDED.pack_ref,
                runtime_ref = EXCLUDED.runtime_ref,
                runtime_version = EXCLUDED.runtime_version,
                runtime_version_text = EXCLUDED.runtime_version_text,
                env_path = EXCLUDED.env_path,
                manifest_checksum = EXCLUDED.manifest_checksum,
                status = CASE
                    WHEN pack_environment.env_path IS DISTINCT FROM EXCLUDED.env_path
                        OR pack_environment.manifest_checksum IS DISTINCT FROM EXCLUDED.manifest_checksum
                    THEN 'outdated'::pack_environment_status_enum
                    ELSE pack_environment.status
                END,
                install_error = CASE
                    WHEN pack_environment.env_path IS DISTINCT FROM EXCLUDED.env_path
                        OR pack_environment.manifest_checksum IS DISTINCT FROM EXCLUDED.manifest_checksum
                    THEN NULL ELSE pack_environment.install_error
                END,
                claimed_by_worker = CASE
                    WHEN pack_environment.env_path IS DISTINCT FROM EXCLUDED.env_path
                        OR pack_environment.manifest_checksum IS DISTINCT FROM EXCLUDED.manifest_checksum
                    THEN NULL ELSE pack_environment.claimed_by_worker
                END,
                claim_id = CASE
                    WHEN pack_environment.env_path IS DISTINCT FROM EXCLUDED.env_path
                        OR pack_environment.manifest_checksum IS DISTINCT FROM EXCLUDED.manifest_checksum
                    THEN NULL ELSE pack_environment.claim_id
                END,
                claim_expires_at = CASE
                    WHEN pack_environment.env_path IS DISTINCT FROM EXCLUDED.env_path
                        OR pack_environment.manifest_checksum IS DISTINCT FROM EXCLUDED.manifest_checksum
                    THEN NULL ELSE pack_environment.claim_expires_at
                END,
                updated = NOW()
            RETURNING id, pack, pack_ref, runtime, runtime_ref, runtime_version, runtime_version_text,
                      env_key, env_path, status, manifest_checksum, claimed_by_worker, claim_id,
                      claim_expires_at, installed_at, last_verified, install_log, install_error,
                      metadata
            "#,
        )
        .bind(input.pack_id)
        .bind(input.pack_ref)
        .bind(input.runtime_id)
        .bind(input.runtime_ref)
        .bind(runtime_version_id)
        .bind(runtime_version_text)
        .bind(&env_path_str)
        .bind(input.manifest_checksum)
        .fetch_one(&self.pool)
        .await?;

        Self::coordinated_environment_from_row(&row)
    }

    pub async fn get_coordinated_environment(
        &self,
        env_key: &str,
    ) -> Result<Option<CoordinatedPackEnvironment>> {
        let row = sqlx::query(
            r#"
            SELECT id, pack, pack_ref, runtime, runtime_ref, runtime_version, runtime_version_text,
                   env_key, env_path, status, manifest_checksum, claimed_by_worker, claim_id,
                   claim_expires_at, installed_at, last_verified, install_log, install_error,
                   metadata
            FROM pack_environment
            WHERE env_key = $1
            "#,
        )
        .bind(env_key)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| Self::coordinated_environment_from_row(&row))
            .transpose()
    }

    async fn lock_coordinated_environment(
        &self,
        env_key: &str,
        worker_id: i64,
    ) -> Result<Transaction<'static, Postgres>> {
        let mut transaction = self.pool.begin().await?;
        // Source before target also serializes worker deletion's FK cleanup.
        let _ = sqlx::query_scalar::<_, i64>("SELECT id FROM worker WHERE id=$1 FOR KEY SHARE")
            .bind(worker_id)
            .fetch_optional(&mut *transaction)
            .await?;
        // Evaluate lease clocks only after this wait, in the following UPDATE.
        // The owned transaction also serializes source replacement/takeover.
        let _ = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM pack_environment WHERE env_key=$1 FOR UPDATE",
        )
        .bind(env_key)
        .fetch_optional(&mut *transaction)
        .await?;
        Ok(transaction)
    }

    pub async fn claim_coordinated_environment(
        &self,
        expected: &CoordinatedPackEnvironment,
        worker_id: i64,
        lease_seconds: i64,
    ) -> Result<Option<CoordinatedPackEnvironment>> {
        if lease_seconds <= 0 {
            return Err(Error::validation("Invalid installation lease"));
        }
        let mut transaction = self
            .lock_coordinated_environment(&expected.env_key, worker_id)
            .await?;
        let row = sqlx::query(
            r#"
            UPDATE pack_environment
            SET status = 'installing',
                claimed_by_worker = $2,
                claim_id = gen_random_uuid(),
                claim_expires_at = clock_timestamp() + ($3 * INTERVAL '1 second'),
                install_error = NULL,
                updated = NOW()
            WHERE env_key = $1 AND id = $6
              AND env_path = $4 AND manifest_checksum IS NOT DISTINCT FROM $5
              AND (
                  status IN ('pending', 'failed', 'outdated')
                  OR (status = 'installing' AND (claimed_by_worker IS NULL OR claim_expires_at IS NULL OR claim_expires_at <= clock_timestamp()))
              )
            RETURNING id, pack, pack_ref, runtime, runtime_ref, runtime_version, runtime_version_text,
                      env_key, env_path, status, manifest_checksum, claimed_by_worker, claim_id,
                      claim_expires_at, installed_at, last_verified, install_log, install_error,
                      metadata
            "#,
        )
        .bind(&expected.env_key)
        .bind(worker_id)
        .bind(lease_seconds)
        .bind(&expected.env_path)
        .bind(&expected.manifest_checksum)
        .bind(expected.id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;

        row.map(|row| Self::coordinated_environment_from_row(&row))
            .transpose()
    }

    pub async fn renew_coordinated_environment_claim(
        &self,
        env_key: &str,
        worker_id: i64,
        claim_id: Uuid,
        lease_seconds: i64,
    ) -> Result<bool> {
        if lease_seconds <= 0 {
            return Err(Error::validation("Invalid installation lease"));
        }
        let mut transaction = self
            .lock_coordinated_environment(env_key, worker_id)
            .await?;
        let updated = sqlx::query(
            r#"
            UPDATE pack_environment
            SET claim_expires_at = clock_timestamp() + ($4 * INTERVAL '1 second'),
                updated = NOW()
            WHERE env_key = $1
              AND claimed_by_worker = $2
              AND claim_id = $3
              AND status = 'installing'
              AND claim_expires_at > clock_timestamp()
            "#,
        )
        .bind(env_key)
        .bind(worker_id)
        .bind(claim_id)
        .bind(lease_seconds)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(updated.rows_affected() > 0)
    }

    pub async fn mark_coordinated_environment_ready(
        &self,
        env_key: &str,
        worker_id: i64,
        claim_id: Uuid,
        manifest_checksum: Option<&str>,
    ) -> Result<bool> {
        let mut transaction = self
            .lock_coordinated_environment(env_key, worker_id)
            .await?;
        let updated = sqlx::query(
            r#"
            UPDATE pack_environment
            SET status = 'ready',
                manifest_checksum = $4,
                installed_at = NOW(),
                last_verified = NOW(),
                claimed_by_worker = NULL,
                claim_id = NULL,
                claim_expires_at = NULL,
                install_error = NULL,
                updated = NOW()
            WHERE env_key = $1
              AND claimed_by_worker = $2
              AND claim_id = $3
              AND status = 'installing'
              AND claim_expires_at > clock_timestamp()
            "#,
        )
        .bind(env_key)
        .bind(worker_id)
        .bind(claim_id)
        .bind(manifest_checksum)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(updated.rows_affected() > 0)
    }

    pub async fn mark_coordinated_environment_failed(
        &self,
        env_key: &str,
        worker_id: i64,
        claim_id: Uuid,
        error_message: &str,
    ) -> Result<bool> {
        let mut transaction = self
            .lock_coordinated_environment(env_key, worker_id)
            .await?;
        let updated = sqlx::query(
            r#"
            UPDATE pack_environment
            SET status = 'failed',
                install_error = $4,
                claimed_by_worker = NULL,
                claim_id = NULL,
                claim_expires_at = NULL,
                updated = NOW()
            WHERE env_key = $1
              AND claimed_by_worker = $2
              AND claim_id = $3
              AND status = 'installing'
            "#,
        )
        .bind(env_key)
        .bind(worker_id)
        .bind(claim_id)
        .bind(error_message)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(updated.rows_affected() > 0)
    }

    pub async fn mark_coordinated_environment_outdated(&self, env_key: &str) -> Result<bool> {
        let updated = sqlx::query(
            r#"
            UPDATE pack_environment
            SET status = 'outdated',
                claimed_by_worker = NULL,
                claim_id = NULL,
                claim_expires_at = NULL,
                updated = NOW()
            WHERE env_key = $1
            "#,
        )
        .bind(env_key)
        .execute(&self.pool)
        .await?;

        Ok(updated.rows_affected() > 0)
    }
}

/// Collect the lowercase runtime names that require environment setup for a pack.
///
/// This queries the pack's actions, resolves their runtimes, and returns the names
/// of any runtimes that have environment or dependency configuration. It is used by
/// the API when publishing `PackRegistered` MQ events so that workers know which
/// runtimes to set up without re-querying the database.
pub async fn collect_runtime_names_for_pack(
    db_pool: &PgPool,
    pack_id: i64,
    pack_path: &Path,
) -> Vec<String> {
    let actions = match ActionRepository::find_by_pack(db_pool, pack_id).await {
        Ok(a) => a,
        Err(e) => {
            warn!("Failed to load actions for pack ID {}: {}", pack_id, e);
            return Vec::new();
        }
    };

    let mut seen_runtime_ids = HashSet::new();
    for action in &actions {
        if let Some(runtime_id) = action.runtime {
            seen_runtime_ids.insert(runtime_id);
        }
    }

    let mut runtime_names = Vec::new();
    for runtime_id in seen_runtime_ids {
        match RuntimeRepository::find_by_id(db_pool, runtime_id).await {
            Ok(Some(rt)) => {
                let exec_config = rt.parsed_execution_config();
                if exec_config.environment.is_some() || exec_config.has_dependencies(pack_path) {
                    runtime_names.push(rt.name.to_lowercase());
                }
            }
            Ok(None) => {
                debug!("Runtime ID {} not found, skipping", runtime_id);
            }
            Err(e) => {
                warn!("Failed to load runtime {}: {}", runtime_id, e);
            }
        }
    }

    for action in &actions {
        for requirement in action.required_worker_runtime_constraints().into_keys() {
            let normalized_requirement = normalize_runtime_name(&requirement);
            let runtime = match RuntimeRepository::find_by_alias(db_pool, &normalized_requirement)
                .await
            {
                Ok(Some(runtime)) => Some(runtime),
                Ok(None) => {
                    match RuntimeRepository::find_by_name(db_pool, &normalized_requirement).await {
                        Ok(Some(runtime)) => Some(runtime),
                        Ok(None) => RuntimeRepository::find_by_ref(db_pool, &requirement)
                            .await
                            .ok()
                            .flatten(),
                        Err(e) => {
                            warn!(
                                "Failed to resolve runtime requirement '{}' by name: {}",
                                requirement, e
                            );
                            None
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to resolve runtime requirement '{}' by alias: {}",
                        requirement, e
                    );
                    None
                }
            };

            if let Some(rt) = runtime {
                let exec_config = rt.parsed_execution_config();
                let runtime_name = rt.name.to_lowercase();
                if (exec_config.environment.is_some() || exec_config.has_dependencies(pack_path))
                    && !runtime_names.contains(&runtime_name)
                {
                    runtime_names.push(runtime_name);
                }
            }
        }
    }

    runtime_names
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_environment_status_conversion() {
        assert_eq!(PackEnvironmentStatus::Ready.as_str(), "ready");
        assert_eq!(
            PackEnvironmentStatus::parse_status("ready"),
            Some(PackEnvironmentStatus::Ready)
        );
        assert_eq!(PackEnvironmentStatus::parse_status("invalid"), None);
    }
}
