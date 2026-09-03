//! Repository for runtime version operations
//!
//! Provides CRUD operations and specialized queries for the `runtime_version`
//! table, which stores version-specific execution configurations for runtimes.

use crate::error::Result;
use crate::models::{Id, RuntimeVersion};
use crate::repositories::{Create, Delete, FindById, List, Patch, Repository, Update};
use sqlx::{Executor, Postgres, QueryBuilder};

/// Repository for runtime version database operations
pub struct RuntimeVersionRepository;

impl Repository for RuntimeVersionRepository {
    type Entity = RuntimeVersion;

    fn table_name() -> &'static str {
        "runtime_version"
    }
}

/// Input for creating a new runtime version
#[derive(Debug, Clone)]
pub struct CreateRuntimeVersionInput {
    pub runtime: Id,
    pub runtime_ref: String,
    pub version: String,
    pub version_major: Option<i32>,
    pub version_minor: Option<i32>,
    pub version_patch: Option<i32>,
    pub execution_config: serde_json::Value,
    pub distributions: serde_json::Value,
    pub is_default: bool,
    pub available: bool,
    pub meta: serde_json::Value,
}

/// Input for updating an existing runtime version
#[derive(Debug, Clone, Default)]
pub struct UpdateRuntimeVersionInput {
    pub version: Option<String>,
    pub version_major: Option<Patch<i32>>,
    pub version_minor: Option<Patch<i32>>,
    pub version_patch: Option<Patch<i32>>,
    pub execution_config: Option<serde_json::Value>,
    pub distributions: Option<serde_json::Value>,
    pub is_default: Option<bool>,
    pub available: Option<bool>,
    pub verified_at: Option<Patch<chrono::DateTime<chrono::Utc>>>,
    pub meta: Option<serde_json::Value>,
}

pub(crate) const SELECT_COLUMNS: &str = r#"
    id, runtime, runtime_ref, version,
    version_major, version_minor, version_patch,
    execution_config, distributions,
    is_default, available, verified_at, meta,
    created, updated
"#;

#[async_trait::async_trait]
impl FindById for RuntimeVersionRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<RuntimeVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let row = sqlx::query_as::<_, RuntimeVersion>(&format!(
            "SELECT {} FROM runtime_version WHERE id = $1",
            SELECT_COLUMNS
        ))
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(row)
    }
}

#[async_trait::async_trait]
impl List for RuntimeVersionRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<RuntimeVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rows = sqlx::query_as::<_, RuntimeVersion>(&format!(
            "SELECT {} FROM runtime_version ORDER BY runtime_ref ASC, version ASC",
            SELECT_COLUMNS
        ))
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }
}

#[async_trait::async_trait]
impl Create for RuntimeVersionRepository {
    type CreateInput = CreateRuntimeVersionInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<RuntimeVersion>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let row = sqlx::query_as::<_, RuntimeVersion>(&format!(
            r#"
            INSERT INTO runtime_version (
                runtime, runtime_ref, version,
                version_major, version_minor, version_patch,
                execution_config, distributions,
                is_default, available, meta
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING {}
            "#,
            SELECT_COLUMNS
        ))
        .bind(input.runtime)
        .bind(&input.runtime_ref)
        .bind(&input.version)
        .bind(input.version_major)
        .bind(input.version_minor)
        .bind(input.version_patch)
        .bind(&input.execution_config)
        .bind(&input.distributions)
        .bind(input.is_default)
        .bind(input.available)
        .bind(&input.meta)
        .fetch_one(executor)
        .await?;

        Ok(row)
    }
}

#[async_trait::async_trait]
impl Update for RuntimeVersionRepository {
    type UpdateInput = UpdateRuntimeVersionInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<RuntimeVersion>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE runtime_version SET ");
        let mut has_updates = false;

        if let Some(version) = &input.version {
            query.push("version = ");
            query.push_bind(version);
            has_updates = true;
        }

        if let Some(version_major) = &input.version_major {
            if has_updates {
                query.push(", ");
            }
            query.push("version_major = ");
            match version_major {
                Patch::Set(value) => query.push_bind(*value),
                Patch::Clear => query.push_bind(Option::<i32>::None),
            };
            has_updates = true;
        }

        if let Some(version_minor) = &input.version_minor {
            if has_updates {
                query.push(", ");
            }
            query.push("version_minor = ");
            match version_minor {
                Patch::Set(value) => query.push_bind(*value),
                Patch::Clear => query.push_bind(Option::<i32>::None),
            };
            has_updates = true;
        }

        if let Some(version_patch) = &input.version_patch {
            if has_updates {
                query.push(", ");
            }
            query.push("version_patch = ");
            match version_patch {
                Patch::Set(value) => query.push_bind(*value),
                Patch::Clear => query.push_bind(Option::<i32>::None),
            };
            has_updates = true;
        }

        if let Some(execution_config) = &input.execution_config {
            if has_updates {
                query.push(", ");
            }
            query.push("execution_config = ");
            query.push_bind(execution_config);
            has_updates = true;
        }

        if let Some(distributions) = &input.distributions {
            if has_updates {
                query.push(", ");
            }
            query.push("distributions = ");
            query.push_bind(distributions);
            has_updates = true;
        }

        if let Some(is_default) = input.is_default {
            if has_updates {
                query.push(", ");
            }
            query.push("is_default = ");
            query.push_bind(is_default);
            has_updates = true;
        }

        if let Some(available) = input.available {
            if has_updates {
                query.push(", ");
            }
            query.push("available = ");
            query.push_bind(available);
            has_updates = true;
        }

        if let Some(verified_at) = &input.verified_at {
            if has_updates {
                query.push(", ");
            }
            query.push("verified_at = ");
            match verified_at {
                Patch::Set(value) => query.push_bind(*value),
                Patch::Clear => query.push_bind(Option::<chrono::DateTime<chrono::Utc>>::None),
            };
            has_updates = true;
        }

        if let Some(meta) = &input.meta {
            if has_updates {
                query.push(", ");
            }
            query.push("meta = ");
            query.push_bind(meta);
            has_updates = true;
        }

        if !has_updates {
            // Nothing to update — just fetch the current row
            return Self::find_by_id(executor, id)
                .await?
                .ok_or_else(|| crate::Error::not_found("runtime_version", "id", id.to_string()));
        }

        query.push(" WHERE id = ");
        query.push_bind(id);
        query.push(format!(" RETURNING {}", SELECT_COLUMNS));

        let row = query
            .build_query_as::<RuntimeVersion>()
            .fetch_optional(executor)
            .await?
            .ok_or_else(|| crate::Error::not_found("runtime_version", "id", id.to_string()))?;

        Ok(row)
    }
}

#[async_trait::async_trait]
impl Delete for RuntimeVersionRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM runtime_version WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

/// Specialized queries
impl RuntimeVersionRepository {
    /// Find all versions for a given runtime ID.
    ///
    /// Returns versions ordered by major, minor, patch descending
    /// (newest version first).
    pub async fn find_by_runtime<'e, E>(executor: E, runtime_id: Id) -> Result<Vec<RuntimeVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rows = sqlx::query_as::<_, RuntimeVersion>(&format!(
            r#"
            SELECT {}
            FROM runtime_version
            WHERE runtime = $1
            ORDER BY version_major DESC NULLS LAST,
                     version_minor DESC NULLS LAST,
                     version_patch DESC NULLS LAST
            "#,
            SELECT_COLUMNS
        ))
        .bind(runtime_id)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    /// Find all versions for a given runtime ref (e.g., "core.python").
    pub async fn find_by_runtime_ref<'e, E>(
        executor: E,
        runtime_ref: &str,
    ) -> Result<Vec<RuntimeVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rows = sqlx::query_as::<_, RuntimeVersion>(&format!(
            r#"
            SELECT {}
            FROM runtime_version
            WHERE runtime_ref = $1
            ORDER BY version_major DESC NULLS LAST,
                     version_minor DESC NULLS LAST,
                     version_patch DESC NULLS LAST
            "#,
            SELECT_COLUMNS
        ))
        .bind(runtime_ref)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    /// Find all available versions for a given runtime ID.
    ///
    /// Only returns versions where `available = true`.
    pub async fn find_available_by_runtime<'e, E>(
        executor: E,
        runtime_id: Id,
    ) -> Result<Vec<RuntimeVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rows = sqlx::query_as::<_, RuntimeVersion>(&format!(
            r#"
            SELECT {}
            FROM runtime_version
            WHERE runtime = $1 AND available = TRUE
            ORDER BY version_major DESC NULLS LAST,
                     version_minor DESC NULLS LAST,
                     version_patch DESC NULLS LAST
            "#,
            SELECT_COLUMNS
        ))
        .bind(runtime_id)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    /// Find the default version for a given runtime ID.
    ///
    /// Returns `None` if no version is marked as default.
    pub async fn find_default_by_runtime<'e, E>(
        executor: E,
        runtime_id: Id,
    ) -> Result<Option<RuntimeVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let row = sqlx::query_as::<_, RuntimeVersion>(&format!(
            r#"
            SELECT {}
            FROM runtime_version
            WHERE runtime = $1 AND is_default = TRUE
            LIMIT 1
            "#,
            SELECT_COLUMNS
        ))
        .bind(runtime_id)
        .fetch_optional(executor)
        .await?;

        Ok(row)
    }

    /// Find a specific version by runtime ID and version string.
    pub async fn find_by_runtime_and_version<'e, E>(
        executor: E,
        runtime_id: Id,
        version: &str,
    ) -> Result<Option<RuntimeVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let row = sqlx::query_as::<_, RuntimeVersion>(&format!(
            r#"
            SELECT {}
            FROM runtime_version
            WHERE runtime = $1 AND version = $2
            "#,
            SELECT_COLUMNS
        ))
        .bind(runtime_id)
        .bind(version)
        .fetch_optional(executor)
        .await?;

        Ok(row)
    }

    /// Clear the `is_default` flag on all versions for a runtime.
    ///
    /// Useful before setting a new default version.
    pub async fn clear_default_for_runtime<'e, E>(executor: E, runtime_id: Id) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            "UPDATE runtime_version SET is_default = FALSE WHERE runtime = $1 AND is_default = TRUE",
        )
        .bind(runtime_id)
        .execute(executor)
        .await?;

        Ok(result.rows_affected())
    }

    /// Mark a version's availability and update the verification timestamp.
    pub async fn set_availability<'e, E>(executor: E, id: Id, available: bool) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            "UPDATE runtime_version SET available = $1, verified_at = NOW() WHERE id = $2",
        )
        .bind(available)
        .bind(id)
        .execute(executor)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Delete all versions for a given runtime ID.
    pub async fn delete_by_runtime<'e, E>(executor: E, runtime_id: Id) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM runtime_version WHERE runtime = $1")
            .bind(runtime_id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected())
    }
}
