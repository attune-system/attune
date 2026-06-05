//! Runtime and Worker repository for database operations
//!
//! This module provides CRUD operations and queries for Runtime and Worker entities.

use crate::models::{
    enums::{WorkerStatus, WorkerType},
    runtime::*,
    Id, JsonDict,
};
use crate::Result;
use sqlx::{Executor, Postgres, QueryBuilder};

use super::{Create, Delete, FindById, FindByRef, List, Patch, Repository, Update};

/// Repository for Runtime operations
pub struct RuntimeRepository;

impl Repository for RuntimeRepository {
    type Entity = Runtime;

    fn table_name() -> &'static str {
        "runtime"
    }
}

/// Columns selected for all Runtime queries. Centralised here so that
/// schema changes only need one update.
pub const SELECT_COLUMNS: &str = "id, ref, pack, pack_ref, description, name, aliases, \
     distributions, installation, installers, execution_config, \
     auto_detected, detection_config, \
     created, updated";

/// Input for creating a new runtime
#[derive(Debug, Clone)]
pub struct CreateRuntimeInput {
    pub r#ref: String,
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub description: Option<String>,
    pub name: String,
    pub aliases: Vec<String>,
    pub distributions: JsonDict,
    pub installation: Option<JsonDict>,
    pub execution_config: JsonDict,
    pub auto_detected: bool,
    pub detection_config: JsonDict,
}

/// Input for updating a runtime
#[derive(Debug, Clone, Default)]
pub struct UpdateRuntimeInput {
    pub description: Option<Patch<String>>,
    pub name: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub distributions: Option<JsonDict>,
    pub installation: Option<Patch<JsonDict>>,
    pub execution_config: Option<JsonDict>,
    pub auto_detected: Option<bool>,
    pub detection_config: Option<JsonDict>,
}

#[async_trait::async_trait]
impl FindById for RuntimeRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM runtime WHERE id = $1", SELECT_COLUMNS);
        let runtime = sqlx::query_as::<_, Runtime>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await?;

        Ok(runtime)
    }
}

#[async_trait::async_trait]
impl FindByRef for RuntimeRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM runtime WHERE ref = $1", SELECT_COLUMNS);
        let runtime = sqlx::query_as::<_, Runtime>(&query)
            .bind(ref_str)
            .fetch_optional(executor)
            .await?;

        Ok(runtime)
    }
}

#[async_trait::async_trait]
impl List for RuntimeRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM runtime ORDER BY ref ASC", SELECT_COLUMNS);
        let runtimes = sqlx::query_as::<_, Runtime>(&query)
            .fetch_all(executor)
            .await?;

        Ok(runtimes)
    }
}

#[async_trait::async_trait]
impl Create for RuntimeRepository {
    type CreateInput = CreateRuntimeInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "INSERT INTO runtime (ref, pack, pack_ref, description, name, aliases, \
             distributions, installation, installers, execution_config, \
             auto_detected, detection_config) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             RETURNING {}",
            SELECT_COLUMNS
        );
        let runtime = sqlx::query_as::<_, Runtime>(&query)
            .bind(&input.r#ref)
            .bind(input.pack)
            .bind(&input.pack_ref)
            .bind(&input.description)
            .bind(&input.name)
            .bind(&input.aliases)
            .bind(&input.distributions)
            .bind(&input.installation)
            .bind(serde_json::json!({}))
            .bind(&input.execution_config)
            .bind(input.auto_detected)
            .bind(&input.detection_config)
            .fetch_one(executor)
            .await?;

        Ok(runtime)
    }
}

#[async_trait::async_trait]
impl Update for RuntimeRepository {
    type UpdateInput = UpdateRuntimeInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build update query

        let mut query = QueryBuilder::new("UPDATE runtime SET ");
        let mut has_updates = false;

        if let Some(description) = &input.description {
            query.push("description = ");
            match description {
                Patch::Set(description) => query.push_bind(description),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }

        if let Some(name) = &input.name {
            if has_updates {
                query.push(", ");
            }
            query.push("name = ");
            query.push_bind(name);
            has_updates = true;
        }

        if let Some(aliases) = &input.aliases {
            if has_updates {
                query.push(", ");
            }
            query.push("aliases = ");
            query.push_bind(aliases.as_slice());
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

        if let Some(installation) = &input.installation {
            if has_updates {
                query.push(", ");
            }
            query.push("installation = ");
            match installation {
                Patch::Set(installation) => query.push_bind(installation),
                Patch::Clear => query.push_bind(Option::<JsonDict>::None),
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

        if let Some(auto_detected) = input.auto_detected {
            if has_updates {
                query.push(", ");
            }
            query.push("auto_detected = ");
            query.push_bind(auto_detected);
            has_updates = true;
        }

        if let Some(detection_config) = &input.detection_config {
            if has_updates {
                query.push(", ");
            }
            query.push("detection_config = ");
            query.push_bind(detection_config);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ");
        query.push_bind(id);
        query.push(format!(" RETURNING {}", SELECT_COLUMNS));

        let runtime = query
            .build_query_as::<Runtime>()
            .fetch_one(executor)
            .await?;

        Ok(runtime)
    }
}

#[async_trait::async_trait]
impl Delete for RuntimeRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM runtime WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl RuntimeRepository {
    /// Find runtimes by pack
    pub async fn find_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Vec<Runtime>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM runtime WHERE pack = $1 ORDER BY ref ASC",
            SELECT_COLUMNS
        );
        let runtimes = sqlx::query_as::<_, Runtime>(&query)
            .bind(pack_id)
            .fetch_all(executor)
            .await?;

        Ok(runtimes)
    }

    /// Bulk-fetch runtime refs for the given IDs. Returns a map from ID to ref.
    pub async fn find_refs_by_ids<'e, E>(
        executor: E,
        ids: &[Id],
    ) -> Result<std::collections::HashMap<Id, String>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let rows: Vec<(Id, String)> =
            sqlx::query_as("SELECT id, ref FROM runtime WHERE id = ANY($1)")
                .bind(ids)
                .fetch_all(executor)
                .await?;
        Ok(rows.into_iter().collect())
    }

    /// Find a runtime by name (case-insensitive)
    pub async fn find_by_name<'e, E>(executor: E, name: &str) -> Result<Option<Runtime>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM runtime WHERE LOWER(name) = LOWER($1) LIMIT 1",
            SELECT_COLUMNS
        );
        let runtime = sqlx::query_as::<_, Runtime>(&query)
            .bind(name)
            .fetch_optional(executor)
            .await?;

        Ok(runtime)
    }

    /// Find a runtime where the given alias appears in its `aliases` array.
    /// Uses PostgreSQL's `@>` (array contains) operator with a GIN index.
    pub async fn find_by_alias<'e, E>(executor: E, alias: &str) -> Result<Option<Runtime>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM runtime WHERE aliases @> ARRAY[$1]::text[] LIMIT 1",
            SELECT_COLUMNS
        );
        let runtime = sqlx::query_as::<_, Runtime>(&query)
            .bind(alias)
            .fetch_optional(executor)
            .await?;
        Ok(runtime)
    }

    /// Delete runtimes belonging to a pack whose refs are NOT in the given set.
    ///
    /// Used during pack reinstallation to clean up runtimes that were removed
    /// from the pack's YAML files. Associated runtime_version rows are
    /// cascade-deleted by the FK constraint.
    pub async fn delete_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        keep_refs: &[String],
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = if keep_refs.is_empty() {
            sqlx::query("DELETE FROM runtime WHERE pack = $1")
                .bind(pack_id)
                .execute(executor)
                .await?
        } else {
            sqlx::query("DELETE FROM runtime WHERE pack = $1 AND ref != ALL($2)")
                .bind(pack_id)
                .bind(keep_refs)
                .execute(executor)
                .await?
        };

        Ok(result.rows_affected())
    }
}

// ============================================================================
// Worker Repository
// ============================================================================

/// Repository for Worker operations
pub struct WorkerRepository;

impl Repository for WorkerRepository {
    type Entity = Worker;

    fn table_name() -> &'static str {
        "worker"
    }
}

/// Input for creating a new worker
#[derive(Debug, Clone)]
pub struct CreateWorkerInput {
    pub name: String,
    pub worker_type: WorkerType,
    pub runtime: Option<Id>,
    pub host: Option<String>,
    pub port: Option<i32>,
    pub status: Option<WorkerStatus>,
    pub capabilities: Option<JsonDict>,
    pub meta: Option<JsonDict>,
}

/// Input for updating a worker
#[derive(Debug, Clone, Default)]
pub struct UpdateWorkerInput {
    pub name: Option<String>,
    pub status: Option<WorkerStatus>,
    pub capabilities: Option<JsonDict>,
    pub meta: Option<JsonDict>,
    pub host: Option<String>,
    pub port: Option<i32>,
}

const WORKER_SELECT_COLUMNS: &str =
    "id, name, worker_type, worker_role, runtime, host, port, status, \
     capabilities, meta, last_heartbeat, cordoned, cordon_reason, cordoned_by, cordoned_at, \
     created, updated";

#[async_trait::async_trait]
impl FindById for WorkerRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let worker = sqlx::query_as::<_, Worker>(&format!(
            "SELECT {WORKER_SELECT_COLUMNS} FROM worker WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(worker)
    }
}

#[async_trait::async_trait]
impl List for WorkerRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let workers = sqlx::query_as::<_, Worker>(&format!(
            "SELECT {WORKER_SELECT_COLUMNS} FROM worker ORDER BY name ASC"
        ))
        .fetch_all(executor)
        .await?;

        Ok(workers)
    }
}

#[async_trait::async_trait]
impl Create for WorkerRepository {
    type CreateInput = CreateWorkerInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let worker = sqlx::query_as::<_, Worker>(&format!(
            "INSERT INTO worker (name, worker_type, worker_role, runtime, host, port, status, \
                 capabilities, meta, cordoned, cordon_reason, cordoned_by, cordoned_at) \
                 VALUES ($1, $2, 'action', $3, $4, $5, $6, $7, $8, FALSE, NULL, NULL, NULL) \
                 RETURNING {WORKER_SELECT_COLUMNS}"
        ))
        .bind(&input.name)
        .bind(input.worker_type)
        .bind(input.runtime)
        .bind(&input.host)
        .bind(input.port)
        .bind(input.status)
        .bind(&input.capabilities)
        .bind(&input.meta)
        .fetch_one(executor)
        .await?;

        Ok(worker)
    }
}

#[async_trait::async_trait]
impl Update for WorkerRepository {
    type UpdateInput = UpdateWorkerInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build update query

        let mut query = QueryBuilder::new("UPDATE worker SET ");
        let mut has_updates = false;

        if let Some(name) = &input.name {
            query.push("name = ");
            query.push_bind(name);
            has_updates = true;
        }

        if let Some(status) = input.status {
            if has_updates {
                query.push(", ");
            }
            query.push("status = ");
            query.push_bind(status);
            has_updates = true;
        }

        if let Some(capabilities) = &input.capabilities {
            if has_updates {
                query.push(", ");
            }
            query.push("capabilities = ");
            query.push_bind(capabilities);
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

        if let Some(host) = &input.host {
            if has_updates {
                query.push(", ");
            }
            query.push("host = ");
            query.push_bind(host);
            has_updates = true;
        }

        if let Some(port) = input.port {
            if has_updates {
                query.push(", ");
            }
            query.push("port = ");
            query.push_bind(port);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ");
        query.push_bind(id);
        query.push(" RETURNING ");
        query.push(WORKER_SELECT_COLUMNS);

        let worker = query.build_query_as::<Worker>().fetch_one(executor).await?;

        Ok(worker)
    }
}

#[async_trait::async_trait]
impl Delete for WorkerRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM worker WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl WorkerRepository {
    pub async fn set_cordoned(
        pool: &sqlx::PgPool,
        id: Id,
        cordoned: bool,
        reason: Option<String>,
        cordoned_by: Option<Id>,
    ) -> Result<Worker> {
        let worker = sqlx::query_as::<_, Worker>(&format!(
            "UPDATE worker \
             SET cordoned = $1, \
                 cordon_reason = $2, \
                 cordoned_by = $3, \
                 cordoned_at = CASE WHEN $1 THEN NOW() ELSE NULL END, \
                 updated = NOW() \
             WHERE id = $4 \
             RETURNING {WORKER_SELECT_COLUMNS}"
        ))
        .bind(cordoned)
        .bind(reason)
        .bind(cordoned_by)
        .bind(id)
        .fetch_one(pool)
        .await?;

        Ok(worker)
    }

    /// Find workers by status
    pub async fn find_by_status<'e, E>(executor: E, status: WorkerStatus) -> Result<Vec<Worker>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let workers = sqlx::query_as::<_, Worker>(&format!(
            "SELECT {WORKER_SELECT_COLUMNS} FROM worker WHERE status = $1 ORDER BY name ASC"
        ))
        .bind(status)
        .fetch_all(executor)
        .await?;

        Ok(workers)
    }

    /// Find workers by type
    pub async fn find_by_type<'e, E>(executor: E, worker_type: WorkerType) -> Result<Vec<Worker>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let workers = sqlx::query_as::<_, Worker>(&format!(
            "SELECT {WORKER_SELECT_COLUMNS} FROM worker WHERE worker_type = $1 ORDER BY name ASC"
        ))
        .bind(worker_type)
        .fetch_all(executor)
        .await?;

        Ok(workers)
    }

    /// Update worker heartbeat
    pub async fn update_heartbeat<'e, E>(executor: E, id: i64) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query(
            "UPDATE worker SET last_heartbeat = NOW(), status = $1, updated = NOW() WHERE id = $2",
        )
        .bind(WorkerStatus::Active)
        .bind(id)
        .execute(executor)
        .await?;

        Ok(())
    }

    /// Find workers by name
    pub async fn find_by_name<'e, E>(executor: E, name: &str) -> Result<Option<Worker>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let worker = sqlx::query_as::<_, Worker>(&format!(
            "SELECT {WORKER_SELECT_COLUMNS} FROM worker WHERE name = $1"
        ))
        .bind(name)
        .fetch_optional(executor)
        .await?;

        Ok(worker)
    }

    /// Find workers that can execute actions (role = 'action')
    pub async fn find_action_workers<'e, E>(executor: E) -> Result<Vec<Worker>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let workers = sqlx::query_as::<_, Worker>(
            &format!(
                "SELECT {WORKER_SELECT_COLUMNS} FROM worker WHERE worker_role = 'action' ORDER BY name ASC"
            ),
        )
        .fetch_all(executor)
        .await?;

        Ok(workers)
    }
}
