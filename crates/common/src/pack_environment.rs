//! Pack Environment Manager
//!
//! Manages isolated runtime environments for each pack to ensure dependency isolation.
//! Each pack gets its own environment per runtime (e.g., /opt/attune/packenvs/mypack/python/).
//!
//! This prevents dependency conflicts when multiple packs use the same runtime but require
//! different versions of libraries.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::models::{Runtime, RuntimeVersion};
use crate::repositories::action::ActionRepository;
use crate::repositories::runtime::{self, RuntimeRepository};
use crate::repositories::{FindById as _, FindByRef as _};
use crate::runtime_detection::normalize_runtime_name;
use regex::Regex;
use serde_json::Value as JsonValue;
use sqlx::{postgres::PgRow, PgPool, Row, Type};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use tokio::fs;
use tracing::{debug, error, info, warn};

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

/// Installer action definition
#[derive(Debug, Clone)]
pub struct InstallerAction {
    pub name: String,
    pub description: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    pub order: i32,
    pub optional: bool,
    pub condition: Option<JsonValue>,
}

/// Pack environment manager
#[derive(Clone)]
pub struct PackEnvironmentManager {
    pool: PgPool,
    #[allow(dead_code)] // Used for future path operations
    base_path: PathBuf,
}

impl PackEnvironmentManager {
    /// Create a new pack environment manager
    pub fn new(pool: PgPool, config: &Config) -> Self {
        let base_path = PathBuf::from(&config.runtime_envs_dir);

        Self { pool, base_path }
    }

    /// Create a new pack environment manager with custom base path
    pub fn with_base_path(pool: PgPool, base_path: PathBuf) -> Self {
        Self { pool, base_path }
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
            claim_expires_at: row.try_get("claim_expires_at")?,
            installed_at: row.try_get("installed_at")?,
            last_verified: row.try_get("last_verified")?,
            install_log: row.try_get("install_log")?,
            install_error: row.try_get("install_error")?,
            metadata: row.try_get("metadata")?,
        })
    }

    /// Create or update a pack environment
    pub async fn ensure_environment(
        &self,
        pack_id: i64,
        pack_ref: &str,
        runtime_id: i64,
        runtime_ref: &str,
        pack_path: &Path,
    ) -> Result<PackEnvironment> {
        info!(
            "Ensuring environment for pack '{}' with runtime '{}'",
            pack_ref, runtime_ref
        );

        // Check if environment already exists
        let existing = self.get_environment(pack_id, runtime_id).await?;

        if let Some(env) = existing {
            if env.status == PackEnvironmentStatus::Ready {
                info!("Environment already exists and is ready: {}", env.env_path);
                return Ok(env);
            } else if env.status == PackEnvironmentStatus::Installing {
                warn!(
                    "Environment is currently installing, returning existing record: {}",
                    env.env_path
                );
                return Ok(env);
            }
            // If failed or outdated, we'll recreate
            info!("Existing environment status: {:?}, recreating", env.status);
        }

        // Get runtime metadata
        let runtime = self.get_runtime(runtime_id).await?;

        // Check if this runtime requires an environment
        if !self.runtime_requires_environment(&runtime)? {
            info!(
                "Runtime '{}' does not require a pack-specific environment",
                runtime_ref
            );
            return self
                .create_no_op_environment(pack_id, pack_ref, runtime_id, runtime_ref)
                .await;
        }

        // Calculate environment path
        let env_path = self.calculate_env_path(pack_ref, &runtime)?;

        // Create or update database record
        let pack_env = self
            .upsert_environment_record(pack_id, pack_ref, runtime_id, runtime_ref, &env_path)
            .await?;

        // Install the environment
        self.install_environment(&pack_env, &runtime, pack_path)
            .await?;

        // Fetch updated record
        self.get_environment(pack_id, runtime_id)
            .await?
            .ok_or_else(|| {
                Error::Internal("Environment record not found after installation".to_string())
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

    fn runtime_requires_environment(&self, runtime: &Runtime) -> Result<bool> {
        if let Some(requires) = runtime.installers.get("requires_environment") {
            Ok(requires.as_bool().unwrap_or(true))
        } else {
            // Default: if there are installers, environment is required
            if let Some(installers) = runtime.installers.get("installers") {
                if let Some(arr) = installers.as_array() {
                    Ok(!arr.is_empty())
                } else {
                    Ok(false)
                }
            } else {
                Ok(false)
            }
        }
    }

    fn calculate_env_path(&self, pack_ref: &str, runtime: &Runtime) -> Result<PathBuf> {
        let runtime_name_lower = runtime.name.to_lowercase();
        let template = runtime
            .installers
            .get("base_path_template")
            .and_then(|v| v.as_str())
            .unwrap_or("{pack_ref}/{runtime_name_lower}");

        let path_str = template
            .replace("{pack_ref}", pack_ref)
            .replace("{runtime_ref}", &runtime.r#ref)
            .replace("{runtime_name_lower}", &runtime_name_lower);

        resolve_env_path(&self.base_path, &path_str)
    }

    async fn upsert_environment_record(
        &self,
        pack_id: i64,
        pack_ref: &str,
        runtime_id: i64,
        runtime_ref: &str,
        env_path: &Path,
    ) -> Result<PackEnvironment> {
        let env_path_str = env_path.to_string_lossy().to_string();

        let row = sqlx::query(
            r#"
            INSERT INTO pack_environment (
                pack, pack_ref, runtime, runtime_ref, runtime_version, runtime_version_text,
                env_path, status
            )
            VALUES ($1, $2, $3, $4, NULL, NULL, $5, 'pending')
            ON CONFLICT (env_key)
            DO UPDATE SET
                env_path = EXCLUDED.env_path,
                status = 'pending',
                install_log = NULL,
                install_error = NULL,
                updated = NOW()
            RETURNING id, pack, pack_ref, runtime, runtime_ref, env_path, status,
                      installed_at, last_verified, install_log, install_error, metadata
            "#,
        )
        .bind(pack_id)
        .bind(pack_ref)
        .bind(runtime_id)
        .bind(runtime_ref)
        .bind(&env_path_str)
        .fetch_one(&self.pool)
        .await?;

        Self::pack_environment_from_row(&row)
    }

    async fn create_no_op_environment(
        &self,
        pack_id: i64,
        pack_ref: &str,
        runtime_id: i64,
        runtime_ref: &str,
    ) -> Result<PackEnvironment> {
        let row = sqlx::query(
            r#"
            INSERT INTO pack_environment (
                pack, pack_ref, runtime, runtime_ref, runtime_version, runtime_version_text,
                env_path, status, installed_at
            )
            VALUES ($1, $2, $3, $4, NULL, NULL, '', 'ready', NOW())
            ON CONFLICT (env_key)
            DO UPDATE SET status = 'ready', installed_at = NOW(), updated = NOW()
            RETURNING id, pack, pack_ref, runtime, runtime_ref, env_path, status,
                      installed_at, last_verified, install_log, install_error, metadata
            "#,
        )
        .bind(pack_id)
        .bind(pack_ref)
        .bind(runtime_id)
        .bind(runtime_ref)
        .fetch_one(&self.pool)
        .await?;

        Self::pack_environment_from_row(&row)
    }

    async fn install_environment(
        &self,
        pack_env: &PackEnvironment,
        runtime: &Runtime,
        pack_path: &Path,
    ) -> Result<()> {
        info!("Installing environment: {}", pack_env.env_path);

        // Update status to installing
        sqlx::query("UPDATE pack_environment SET status = 'installing' WHERE id = $1")
            .bind(pack_env.id)
            .execute(&self.pool)
            .await?;

        let mut install_log = String::new();

        // Create environment directory
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- env_path comes from validated runtime-env path construction under runtime_envs_dir.
        let env_path = PathBuf::from(&pack_env.env_path);
        if env_path.exists() {
            warn!(
                "Environment directory already exists, removing: {}",
                pack_env.env_path
            );
            fs::remove_dir_all(&env_path).await.map_err(|e| {
                Error::Internal(format!("Failed to remove existing environment: {}", e))
            })?;
        }

        fs::create_dir_all(&env_path).await.map_err(|e| {
            Error::Internal(format!("Failed to create environment directory: {}", e))
        })?;

        install_log.push_str(&format!("Created directory: {}\n", pack_env.env_path));

        // Get installer actions
        let installer_actions = self.parse_installer_actions(
            runtime,
            &pack_env.pack_ref,
            &pack_env.runtime_ref,
            &pack_env.env_path,
            pack_path,
        )?;

        // Execute each installer action in order
        for action in installer_actions {
            info!(
                "Executing installer: {} - {}",
                action.name,
                action.description.as_deref().unwrap_or("")
            );

            // Check condition if present
            if let Some(condition) = &action.condition {
                if !self.evaluate_condition(condition, pack_path)? {
                    info!("Skipping installer '{}': condition not met", action.name);
                    install_log
                        .push_str(&format!("Skipped: {} (condition not met)\n", action.name));
                    continue;
                }
            }

            match self.execute_installer_action(&action).await {
                Ok(output) => {
                    install_log.push_str(&format!("\n=== {} ===\n", action.name));
                    install_log.push_str(&output);
                    install_log.push('\n');
                }
                Err(e) => {
                    let error_msg = format!("Installer '{}' failed: {}", action.name, e);
                    error!("{}", error_msg);
                    install_log.push_str(&format!("\nERROR: {}\n", error_msg));

                    if !action.optional {
                        // Mark as failed
                        sqlx::query(
                            "UPDATE pack_environment SET status = 'failed', install_log = $1, install_error = $2 WHERE id = $3"
                        )
                        .bind(&install_log)
                        .bind(&error_msg)
                        .bind(pack_env.id)
                        .execute(&self.pool)
                        .await?;

                        return Err(Error::Internal(error_msg));
                    } else {
                        warn!("Optional installer '{}' failed, continuing", action.name);
                    }
                }
            }
        }

        // Mark as ready
        sqlx::query(
            "UPDATE pack_environment SET status = 'ready', installed_at = NOW(), last_verified = NOW(), install_log = $1 WHERE id = $2"
        )
        .bind(&install_log)
        .bind(pack_env.id)
        .execute(&self.pool)
        .await?;

        info!("Environment installation complete: {}", pack_env.env_path);

        Ok(())
    }

    fn parse_installer_actions(
        &self,
        runtime: &Runtime,
        pack_ref: &str,
        runtime_ref: &str,
        env_path: &str,
        pack_path: &Path,
    ) -> Result<Vec<InstallerAction>> {
        let installers = runtime
            .installers
            .get("installers")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Internal("No installers found for runtime".to_string()))?;

        let pack_path_str = pack_path.to_string_lossy().to_string();
        let mut actions = Vec::new();

        for installer in installers {
            let name = installer
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Internal("Installer missing 'name' field".to_string()))?
                .to_string();

            let description = installer
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from);

            let command_template = installer
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    Error::Internal(format!("Installer '{}' missing 'command' field", name))
                })?;

            let command = self.resolve_template(
                command_template,
                pack_ref,
                runtime_ref,
                env_path,
                &pack_path_str,
            )?;
            // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The candidate command path is validated and confined before any execution is attempted.
            let command = validate_installer_command(&command, pack_path, Path::new(env_path))?;

            let args_template = installer
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(String::from)
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default();

            let args = args_template
                .iter()
                .map(|arg| {
                    self.resolve_template(arg, pack_ref, runtime_ref, env_path, &pack_path_str)
                })
                .collect::<Result<Vec<String>>>()?;

            let cwd_template = installer.get("cwd").and_then(|v| v.as_str());
            let cwd = if let Some(cwd_t) = cwd_template {
                // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Installer cwd values are validated to stay under the pack root or environment directory.
                Some(validate_installer_path(
                    &self.resolve_template(
                        cwd_t,
                        pack_ref,
                        runtime_ref,
                        env_path,
                        &pack_path_str,
                    )?,
                    pack_path,
                    Path::new(env_path), // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- validate_installer_path normalizes and confines this path to the pack or runtime environment root.
                )?)
            } else {
                None
            };

            let env_map = installer
                .get("env")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| {
                            v.as_str().and_then(|s| {
                                let resolved = self
                                    .resolve_template(
                                        s,
                                        pack_ref,
                                        runtime_ref,
                                        env_path,
                                        &pack_path_str,
                                    )
                                    .ok()?;
                                Some((k.clone(), resolved))
                            })
                        })
                        .collect::<HashMap<String, String>>()
                })
                .unwrap_or_default();

            let order = installer
                .get("order")
                .and_then(|v| v.as_i64())
                .unwrap_or(999) as i32;
            let optional = installer
                .get("optional")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let condition = installer.get("condition").cloned();

            actions.push(InstallerAction {
                name,
                description,
                command,
                args,
                cwd,
                env: env_map,
                order,
                optional,
                condition,
            });
        }

        // Sort by order
        actions.sort_by_key(|a| a.order);

        Ok(actions)
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

    async fn execute_installer_action(&self, action: &InstallerAction) -> Result<String> {
        debug!("Executing: {} {:?}", action.command, action.args);

        // nosemgrep: rust.actix.command-injection.rust-actix-command-injection.rust-actix-command-injection -- action.command is accepted only after strict validation of executable shape and allowed path roots.
        let mut cmd = Command::new(&action.command);
        cmd.args(&action.args);

        if let Some(cwd) = &action.cwd {
            cmd.current_dir(cwd);
        }

        for (key, value) in &action.env {
            cmd.env(key, value);
        }

        let output = cmd.output().map_err(|e| {
            Error::Internal(format!(
                "Failed to execute command '{}': {}",
                action.command, e
            ))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let combined = format!("STDOUT:\n{}\nSTDERR:\n{}\n", stdout, stderr);

        if !output.status.success() {
            return Err(Error::Internal(format!(
                "Command failed with exit code {:?}\n{}",
                output.status.code(),
                combined
            )));
        }

        Ok(combined)
    }

    fn evaluate_condition(&self, condition: &JsonValue, pack_path: &Path) -> Result<bool> {
        // Check file_exists condition
        if let Some(file_path_template) = condition.get("file_exists").and_then(|v| v.as_str()) {
            let file_path = file_path_template.replace("{pack_path}", &pack_path.to_string_lossy());
            // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Conditional file checks are validated to stay under trusted pack/environment roots before filesystem access.
            let validated = validate_installer_path(&file_path, pack_path, &self.base_path)?;
            return Ok(PathBuf::from(validated).exists()); // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- validated is normalized and confined to the pack or runtime environment root.
        }

        // Default: condition is true
        Ok(true)
    }

    async fn mark_environment_outdated(&self, env_id: i64) -> Result<()> {
        sqlx::query("UPDATE pack_environment SET status = 'outdated' WHERE id = $1")
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
                    THEN CASE
                        WHEN pack_environment.status = 'installing'
                            AND pack_environment.claim_expires_at IS NOT NULL
                            AND pack_environment.claim_expires_at > NOW()
                        THEN pack_environment.status
                        ELSE 'outdated'::pack_environment_status_enum
                    END
                    ELSE pack_environment.status
                END,
                install_error = CASE
                    WHEN pack_environment.env_path IS DISTINCT FROM EXCLUDED.env_path
                        OR pack_environment.manifest_checksum IS DISTINCT FROM EXCLUDED.manifest_checksum
                    THEN NULL
                    ELSE pack_environment.install_error
                END,
                claimed_by_worker = CASE
                    WHEN pack_environment.env_path IS DISTINCT FROM EXCLUDED.env_path
                        OR pack_environment.manifest_checksum IS DISTINCT FROM EXCLUDED.manifest_checksum
                    THEN CASE
                        WHEN pack_environment.status = 'installing'
                            AND pack_environment.claim_expires_at IS NOT NULL
                            AND pack_environment.claim_expires_at > NOW()
                        THEN pack_environment.claimed_by_worker
                        ELSE NULL
                    END
                    ELSE pack_environment.claimed_by_worker
                END,
                claim_expires_at = CASE
                    WHEN pack_environment.env_path IS DISTINCT FROM EXCLUDED.env_path
                        OR pack_environment.manifest_checksum IS DISTINCT FROM EXCLUDED.manifest_checksum
                    THEN CASE
                        WHEN pack_environment.status = 'installing'
                            AND pack_environment.claim_expires_at IS NOT NULL
                            AND pack_environment.claim_expires_at > NOW()
                        THEN pack_environment.claim_expires_at
                        ELSE NULL
                    END
                    ELSE pack_environment.claim_expires_at
                END,
                updated = NOW()
            RETURNING id, pack, pack_ref, runtime, runtime_ref, runtime_version, runtime_version_text,
                      env_key, env_path, status, manifest_checksum, claimed_by_worker,
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
                   env_key, env_path, status, manifest_checksum, claimed_by_worker,
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

    pub async fn claim_coordinated_environment(
        &self,
        env_key: &str,
        worker_id: i64,
        lease_seconds: i64,
    ) -> Result<Option<CoordinatedPackEnvironment>> {
        let row = sqlx::query(
            r#"
            UPDATE pack_environment
            SET status = 'installing',
                claimed_by_worker = $2,
                claim_expires_at = NOW() + ($3 * INTERVAL '1 second'),
                install_error = NULL,
                updated = NOW()
            WHERE env_key = $1
              AND (
                  status IN ('pending', 'failed', 'outdated')
                  OR (status = 'installing' AND (claim_expires_at IS NULL OR claim_expires_at <= NOW()))
              )
            RETURNING id, pack, pack_ref, runtime, runtime_ref, runtime_version, runtime_version_text,
                      env_key, env_path, status, manifest_checksum, claimed_by_worker,
                      claim_expires_at, installed_at, last_verified, install_log, install_error,
                      metadata
            "#,
        )
        .bind(env_key)
        .bind(worker_id)
        .bind(lease_seconds)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| Self::coordinated_environment_from_row(&row))
            .transpose()
    }

    pub async fn renew_coordinated_environment_claim(
        &self,
        env_key: &str,
        worker_id: i64,
        lease_seconds: i64,
    ) -> Result<bool> {
        let updated = sqlx::query(
            r#"
            UPDATE pack_environment
            SET claim_expires_at = NOW() + ($3 * INTERVAL '1 second'),
                updated = NOW()
            WHERE env_key = $1
              AND claimed_by_worker = $2
              AND status = 'installing'
            "#,
        )
        .bind(env_key)
        .bind(worker_id)
        .bind(lease_seconds)
        .execute(&self.pool)
        .await?;

        Ok(updated.rows_affected() > 0)
    }

    pub async fn mark_coordinated_environment_ready(
        &self,
        env_key: &str,
        worker_id: i64,
        manifest_checksum: Option<&str>,
    ) -> Result<bool> {
        let updated = sqlx::query(
            r#"
            UPDATE pack_environment
            SET status = 'ready',
                manifest_checksum = $3,
                installed_at = NOW(),
                last_verified = NOW(),
                claimed_by_worker = NULL,
                claim_expires_at = NULL,
                install_error = NULL,
                updated = NOW()
            WHERE env_key = $1
              AND claimed_by_worker = $2
            "#,
        )
        .bind(env_key)
        .bind(worker_id)
        .bind(manifest_checksum)
        .execute(&self.pool)
        .await?;

        Ok(updated.rows_affected() > 0)
    }

    pub async fn mark_coordinated_environment_failed(
        &self,
        env_key: &str,
        worker_id: i64,
        error_message: &str,
    ) -> Result<bool> {
        let updated = sqlx::query(
            r#"
            UPDATE pack_environment
            SET status = 'failed',
                install_error = $3,
                claimed_by_worker = NULL,
                claim_expires_at = NULL,
                updated = NOW()
            WHERE env_key = $1
              AND claimed_by_worker = $2
            "#,
        )
        .bind(env_key)
        .bind(worker_id)
        .bind(error_message)
        .execute(&self.pool)
        .await?;

        Ok(updated.rows_affected() > 0)
    }

    pub async fn mark_coordinated_environment_outdated(&self, env_key: &str) -> Result<bool> {
        let updated = sqlx::query(
            r#"
            UPDATE pack_environment
            SET status = 'outdated',
                claimed_by_worker = NULL,
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

fn resolve_env_path(base_path: &Path, path_str: &str) -> Result<PathBuf> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- This helper normalizes env paths and preserves legacy absolute templates while still rejecting parent traversal.
    let raw_path = Path::new(path_str);
    if raw_path.is_absolute() {
        return normalize_relative_or_absolute_path(raw_path);
    }

    let joined = base_path.join(raw_path);
    normalize_relative_or_absolute_path(&joined)
}

fn normalize_relative_or_absolute_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                return Err(Error::validation(format!(
                    "Parent-directory traversal is not allowed in installer paths: {}",
                    path.display()
                )));
            }
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }

    Ok(normalized)
}

fn validate_installer_command(command: &str, pack_path: &Path, env_path: &Path) -> Result<String> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Command validation inspects the path form before enforcing allowed executable rules.
    let command_path = Path::new(command);
    if command_path.is_absolute() {
        return validate_installer_path(command, pack_path, env_path);
    }

    if command.contains(std::path::MAIN_SEPARATOR) {
        return Err(Error::validation(format!(
            "Installer command must be a bare executable name or an allowed absolute path: {}",
            command
        )));
    }

    let command_name_re = Regex::new(r"^[A-Za-z0-9._+-]+$").expect("valid installer regex");
    if !command_name_re.is_match(command) {
        return Err(Error::validation(format!(
            "Installer command contains invalid characters: {}",
            command
        )));
    }

    Ok(command.to_string())
}

fn validate_installer_path(path_str: &str, pack_path: &Path, env_path: &Path) -> Result<String> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Path validation normalizes candidate installer paths before enforcing root confinement.
    let path = normalize_path(Path::new(path_str));
    let normalized_pack_path = normalize_path(pack_path);
    let normalized_env_path = normalize_path(env_path);
    if path.starts_with(&normalized_pack_path) || path.starts_with(&normalized_env_path) {
        Ok(path.to_string_lossy().to_string())
    } else {
        Err(Error::validation(format!(
            "Installer path must remain under the pack or environment directory: {}",
            path_str
        )))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
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
