//! Action and Policy repository for database operations
//!
//! This module provides CRUD operations and queries for Action and Policy entities.

use crate::models::{
    action::*, enums::PolicyMethod, Id, JsonDict, JsonSchema, RetentionPolicyType,
};
use crate::scheduling::{parse_worker_affinity, parse_worker_selector, parse_worker_tolerations};
use crate::version_matching::parse_constraint;
use crate::{Error, Result};
use sqlx::{Executor, Postgres, QueryBuilder};

use super::{Create, Delete, FindById, FindByRef, List, Patch, Repository, Update};

/// Columns selected in all Action queries. Must match the `Action` model's `FromRow` fields.
pub const ACTION_COLUMNS: &str = "id, ref, pack, pack_ref, label, description, entrypoint, \
    runtime, enabled, runtime_version_constraint, required_worker_runtimes, \
    worker_selector, worker_tolerations, worker_affinity, \
    param_schema, out_schema, workflow_def, is_adhoc, accesses_mcp, \
    default_execution_permission_set_refs, \
    log_retention_policy, log_retention_limit, artifact_retention_policy, artifact_retention_limit, \
    timeout_seconds, \
    parameter_delivery, parameter_format, output_format, created, updated";

/// Filters for [`ActionRepository::list_search`].
///
/// All fields are optional and combinable (AND). Pagination is always applied.
#[derive(Debug, Clone, Default)]
pub struct ActionSearchFilters {
    /// Filter by single pack ID
    pub pack: Option<Id>,
    /// Filter by multiple pack IDs (action.pack IN (...)). Combined with `pack` via AND.
    pub packs: Vec<Id>,
    /// Text search across ref, label, description, pack_ref (case-insensitive).
    /// Whitespace-separated tokens are AND-matched (each token must appear in at least one field).
    pub query: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

/// Result of [`ActionRepository::list_search`].
#[derive(Debug)]
pub struct ActionSearchResult {
    pub rows: Vec<Action>,
    pub total: u64,
}

/// Repository for Action operations
pub struct ActionRepository;

/// Escape PostgreSQL `LIKE` wildcards (`%`, `_`) and the escape char itself
/// in user-supplied search input. Without this, a search for `%` would match
/// every row (and `_` nearly every row), enabling cheap full-table scans.
/// Used in conjunction with `LIKE ... ESCAPE '\'`.
fn escape_like_wildcards(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn validate_version_constraint(field_name: &str, constraint: &str) -> Result<()> {
    parse_constraint(constraint).map_err(|e| {
        Error::validation(format!("Invalid {} '{}': {}", field_name, constraint, e))
    })?;
    Ok(())
}

fn validate_required_worker_runtimes(required_worker_runtimes: &JsonDict) -> Result<()> {
    let Some(runtime_versions) = required_worker_runtimes.as_object() else {
        return Err(Error::validation(
            "required_worker_runtimes must be a JSON object mapping runtime names to version constraints",
        ));
    };

    for (runtime_name, constraint) in runtime_versions {
        if runtime_name.trim().is_empty() {
            return Err(Error::validation(
                "required_worker_runtimes keys must be non-empty runtime names",
            ));
        }

        let Some(constraint) = constraint.as_str() else {
            return Err(Error::validation(format!(
                "required_worker_runtimes['{}'] must be a string semver constraint or \"*\"",
                runtime_name
            )));
        };

        if constraint.trim() != "*" {
            validate_version_constraint(
                &format!("required_worker_runtimes['{}']", runtime_name),
                constraint,
            )?;
        }
    }

    Ok(())
}

fn validate_log_retention_limit(limit: i32) -> Result<()> {
    if limit <= 0 {
        return Err(Error::validation(
            "log_retention_limit must be greater than zero",
        ));
    }
    Ok(())
}

fn validate_timeout_seconds(timeout: i32) -> Result<()> {
    if timeout <= 0 {
        return Err(Error::validation(
            "timeout_seconds must be greater than zero",
        ));
    }
    Ok(())
}

impl Repository for ActionRepository {
    type Entity = Action;

    fn table_name() -> &'static str {
        "action"
    }
}

/// Input for creating a new action
#[derive(Debug, Clone)]
pub struct CreateActionInput {
    pub r#ref: String,
    pub pack: Id,
    pub pack_ref: String,
    pub label: String,
    pub description: Option<String>,
    pub entrypoint: String,
    pub runtime: Option<Id>,
    pub enabled: bool,
    pub runtime_version_constraint: Option<String>,
    pub required_worker_runtimes: JsonDict,
    pub worker_selector: JsonDict,
    pub worker_tolerations: JsonDict,
    pub worker_affinity: JsonDict,
    pub param_schema: Option<JsonSchema>,
    pub out_schema: Option<JsonSchema>,
    pub is_adhoc: bool,
    #[doc = "Hint that this action may invoke the MCP server and spawn child executions."]
    pub accesses_mcp: bool,
    pub default_execution_permission_set_refs: Vec<String>,
    pub log_retention_policy: Option<RetentionPolicyType>,
    pub log_retention_limit: Option<i32>,
    pub artifact_retention_policy: Option<RetentionPolicyType>,
    pub artifact_retention_limit: Option<i32>,
    pub timeout_seconds: Option<i32>,
}

/// Input for updating an action
#[derive(Debug, Clone, Default)]
pub struct UpdateActionInput {
    pub label: Option<String>,
    pub description: Option<Patch<String>>,
    pub entrypoint: Option<String>,
    pub runtime: Option<Id>,
    pub enabled: Option<bool>,
    pub runtime_version_constraint: Option<Patch<String>>,
    pub required_worker_runtimes: Option<JsonDict>,
    pub worker_selector: Option<JsonDict>,
    pub worker_tolerations: Option<JsonDict>,
    pub worker_affinity: Option<JsonDict>,
    pub param_schema: Option<JsonSchema>,
    pub out_schema: Option<JsonSchema>,
    pub parameter_delivery: Option<String>,
    pub parameter_format: Option<String>,
    pub output_format: Option<String>,
    pub accesses_mcp: Option<bool>,
    pub default_execution_permission_set_refs: Option<Vec<String>>,
    pub log_retention_policy: Option<Patch<RetentionPolicyType>>,
    pub log_retention_limit: Option<Patch<i32>>,
    pub artifact_retention_policy: Option<Patch<RetentionPolicyType>>,
    pub artifact_retention_limit: Option<Patch<i32>>,
    pub timeout_seconds: Option<Patch<i32>>,
}

#[async_trait::async_trait]
impl FindById for ActionRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let action = sqlx::query_as::<_, Action>(&format!(
            "SELECT {} FROM action WHERE id = $1",
            ACTION_COLUMNS
        ))
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(action)
    }
}

#[async_trait::async_trait]
impl FindByRef for ActionRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let action = sqlx::query_as::<_, Action>(&format!(
            "SELECT {} FROM action WHERE ref = $1",
            ACTION_COLUMNS
        ))
        .bind(ref_str)
        .fetch_optional(executor)
        .await?;

        Ok(action)
    }
}

#[async_trait::async_trait]
impl List for ActionRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let actions = sqlx::query_as::<_, Action>(&format!(
            "SELECT {} FROM action ORDER BY ref ASC",
            ACTION_COLUMNS
        ))
        .fetch_all(executor)
        .await?;

        Ok(actions)
    }
}

#[async_trait::async_trait]
impl Create for ActionRepository {
    type CreateInput = CreateActionInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Validate ref format
        if !input
            .r#ref
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Err(Error::validation(
                "Action ref must contain only alphanumeric characters, dots, underscores, and hyphens",
            ));
        }

        if let Some(runtime_version_constraint) = input.runtime_version_constraint.as_deref() {
            validate_version_constraint("runtime_version_constraint", runtime_version_constraint)?;
        }
        validate_required_worker_runtimes(&input.required_worker_runtimes)?;
        if let Some(limit) = input.log_retention_limit {
            validate_log_retention_limit(limit)?;
        }
        if let Some(limit) = input.artifact_retention_limit {
            validate_log_retention_limit(limit)?;
        }
        if let Some(timeout) = input.timeout_seconds {
            validate_timeout_seconds(timeout)?;
        }
        parse_worker_selector(&input.worker_selector)?;
        parse_worker_tolerations(&input.worker_tolerations)?;
        parse_worker_affinity(&input.worker_affinity)?;

        // Try to insert - database will enforce uniqueness constraint
        let action = sqlx::query_as::<_, Action>(&format!(
            r#"
            INSERT INTO action (ref, pack, pack_ref, label, description, entrypoint,
                                 runtime, enabled, runtime_version_constraint, required_worker_runtimes,
                                 worker_selector, worker_tolerations, worker_affinity,
                                  param_schema, out_schema, is_adhoc, accesses_mcp,
                                  default_execution_permission_set_refs,
                                  log_retention_policy, log_retention_limit,
                                  artifact_retention_policy, artifact_retention_limit, timeout_seconds)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)
            RETURNING {}
            "#,
            ACTION_COLUMNS
        ))
        .bind(&input.r#ref)
        .bind(input.pack)
        .bind(&input.pack_ref)
        .bind(&input.label)
        .bind(&input.description)
        .bind(&input.entrypoint)
        .bind(input.runtime)
        .bind(input.enabled)
        .bind(&input.runtime_version_constraint)
        .bind(&input.required_worker_runtimes)
        .bind(&input.worker_selector)
        .bind(&input.worker_tolerations)
        .bind(&input.worker_affinity)
        .bind(&input.param_schema)
        .bind(&input.out_schema)
        .bind(input.is_adhoc)
        .bind(input.accesses_mcp)
        .bind(&input.default_execution_permission_set_refs)
        .bind(input.log_retention_policy)
        .bind(input.log_retention_limit)
        .bind(input.artifact_retention_policy)
        .bind(input.artifact_retention_limit)
        .bind(input.timeout_seconds)
        .fetch_one(executor)
        .await
        .map_err(|e| {
            // Convert unique constraint violation to AlreadyExists error
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.is_unique_violation() {
                    return Error::already_exists("Action", "ref", &input.r#ref);
                }
            }
            e.into()
        })?;

        Ok(action)
    }
}

#[async_trait::async_trait]
impl Update for ActionRepository {
    type UpdateInput = UpdateActionInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if let Some(Patch::Set(runtime_version_constraint)) = &input.runtime_version_constraint {
            validate_version_constraint("runtime_version_constraint", runtime_version_constraint)?;
        }
        if let Some(required_worker_runtimes) = &input.required_worker_runtimes {
            validate_required_worker_runtimes(required_worker_runtimes)?;
        }
        if let Some(Patch::Set(limit)) = &input.log_retention_limit {
            validate_log_retention_limit(*limit)?;
        }
        if let Some(Patch::Set(limit)) = &input.artifact_retention_limit {
            validate_log_retention_limit(*limit)?;
        }
        if let Some(Patch::Set(timeout)) = &input.timeout_seconds {
            validate_timeout_seconds(*timeout)?;
        }
        if let Some(worker_selector) = &input.worker_selector {
            parse_worker_selector(worker_selector)?;
        }
        if let Some(worker_tolerations) = &input.worker_tolerations {
            parse_worker_tolerations(worker_tolerations)?;
        }
        if let Some(worker_affinity) = &input.worker_affinity {
            parse_worker_affinity(worker_affinity)?;
        }

        // Build dynamic UPDATE query
        let mut query = QueryBuilder::new("UPDATE action SET ");
        let mut has_updates = false;

        if let Some(label) = &input.label {
            if has_updates {
                query.push(", ");
            }
            query.push("label = ");
            query.push_bind(label);
            has_updates = true;
        }

        if let Some(description) = &input.description {
            if has_updates {
                query.push(", ");
            }
            query.push("description = ");
            match description {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }

        if let Some(entrypoint) = &input.entrypoint {
            if has_updates {
                query.push(", ");
            }
            query.push("entrypoint = ");
            query.push_bind(entrypoint);
            has_updates = true;
        }

        if let Some(runtime) = input.runtime {
            if has_updates {
                query.push(", ");
            }
            query.push("runtime = ");
            query.push_bind(runtime);
            has_updates = true;
        }

        if let Some(enabled) = input.enabled {
            if has_updates {
                query.push(", ");
            }
            query.push("enabled = ");
            query.push_bind(enabled);
            has_updates = true;
        }

        if let Some(runtime_version_constraint) = &input.runtime_version_constraint {
            if has_updates {
                query.push(", ");
            }
            query.push("runtime_version_constraint = ");
            match runtime_version_constraint {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }

        if let Some(required_worker_runtimes) = &input.required_worker_runtimes {
            if has_updates {
                query.push(", ");
            }
            query.push("required_worker_runtimes = ");
            query.push_bind(required_worker_runtimes);
            has_updates = true;
        }

        if let Some(worker_selector) = &input.worker_selector {
            if has_updates {
                query.push(", ");
            }
            query.push("worker_selector = ");
            query.push_bind(worker_selector);
            has_updates = true;
        }

        if let Some(worker_tolerations) = &input.worker_tolerations {
            if has_updates {
                query.push(", ");
            }
            query.push("worker_tolerations = ");
            query.push_bind(worker_tolerations);
            has_updates = true;
        }

        if let Some(worker_affinity) = &input.worker_affinity {
            if has_updates {
                query.push(", ");
            }
            query.push("worker_affinity = ");
            query.push_bind(worker_affinity);
            has_updates = true;
        }

        if let Some(param_schema) = &input.param_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("param_schema = ");
            query.push_bind(param_schema);
            has_updates = true;
        }

        if let Some(out_schema) = &input.out_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("out_schema = ");
            query.push_bind(out_schema);
            has_updates = true;
        }

        if let Some(parameter_delivery) = &input.parameter_delivery {
            if has_updates {
                query.push(", ");
            }
            query.push("parameter_delivery = ");
            query.push_bind(parameter_delivery);
            has_updates = true;
        }

        if let Some(parameter_format) = &input.parameter_format {
            if has_updates {
                query.push(", ");
            }
            query.push("parameter_format = ");
            query.push_bind(parameter_format);
            has_updates = true;
        }

        if let Some(output_format) = &input.output_format {
            if has_updates {
                query.push(", ");
            }
            query.push("output_format = ");
            query.push_bind(output_format);
            has_updates = true;
        }

        if let Some(accesses_mcp) = input.accesses_mcp {
            if has_updates {
                query.push(", ");
            }
            query.push("accesses_mcp = ");
            query.push_bind(accesses_mcp);
            has_updates = true;
        }

        if let Some(permission_set_refs) = &input.default_execution_permission_set_refs {
            if has_updates {
                query.push(", ");
            }
            query.push("default_execution_permission_set_refs = ");
            query.push_bind(permission_set_refs);
            has_updates = true;
        }

        if let Some(log_retention_policy) = input.log_retention_policy {
            if has_updates {
                query.push(", ");
            }
            query.push("log_retention_policy = ");
            match log_retention_policy {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<RetentionPolicyType>::None),
            };
            has_updates = true;
        }

        if let Some(log_retention_limit) = input.log_retention_limit {
            if has_updates {
                query.push(", ");
            }
            query.push("log_retention_limit = ");
            match log_retention_limit {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<i32>::None),
            };
            has_updates = true;
        }

        if let Some(artifact_retention_policy) = input.artifact_retention_policy {
            if has_updates {
                query.push(", ");
            }
            query.push("artifact_retention_policy = ");
            match artifact_retention_policy {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<RetentionPolicyType>::None),
            };
            has_updates = true;
        }

        if let Some(artifact_retention_limit) = input.artifact_retention_limit {
            if has_updates {
                query.push(", ");
            }
            query.push("artifact_retention_limit = ");
            match artifact_retention_limit {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<i32>::None),
            };
            has_updates = true;
        }

        if let Some(timeout_seconds) = input.timeout_seconds {
            if has_updates {
                query.push(", ");
            }
            query.push("timeout_seconds = ");
            match timeout_seconds {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<i32>::None),
            };
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing action
            return Self::find_by_id(executor, id)
                .await?
                .ok_or_else(|| Error::not_found("action", "id", id.to_string()));
        }

        query.push(", updated = NOW() WHERE id = ");
        query.push_bind(id);
        query.push(format!(" RETURNING {}", ACTION_COLUMNS));

        let action = query
            .build_query_as::<Action>()
            .fetch_one(executor)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => Error::not_found("action", "id", id.to_string()),
                _ => e.into(),
            })?;

        Ok(action)
    }
}

#[async_trait::async_trait]
impl Delete for ActionRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM action WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl ActionRepository {
    /// Search actions with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    pub async fn list_search<'e, E>(
        db: E,
        filters: &ActionSearchFilters,
    ) -> Result<ActionSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {} FROM action", ACTION_COLUMNS));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM action");

        let mut has_where = false;

        // Combine the single-pack shorthand and the multi-pack filter into one
        // `pack = ANY($N)` clause. Treating them as conjunctive (AND) creates
        // an impossible-constraint footgun if both are set with disjoint values.
        let mut combined_pack_ids: Vec<Id> = filters.packs.clone();
        if let Some(pack_id) = filters.pack {
            if !combined_pack_ids.contains(&pack_id) {
                combined_pack_ids.push(pack_id);
            }
        }
        if !combined_pack_ids.is_empty() {
            if !has_where {
                qb.push(" WHERE ");
                count_qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
                count_qb.push(" AND ");
            }
            qb.push("pack = ANY(");
            qb.push_bind(combined_pack_ids.clone());
            qb.push(")");
            count_qb.push("pack = ANY(");
            count_qb.push_bind(combined_pack_ids);
            count_qb.push(")");
        }
        if let Some(ref query) = filters.query {
            // Tokenize the query: each whitespace-separated token must match
            // at least one of (ref, label, description, pack_ref). This lets
            // callers find an action with multi-keyword searches like
            // "slack post message" without caring about field ordering.
            //
            // Cap token count to bound query cost and prevent pathological
            // inputs from generating huge OR/AND trees.
            const MAX_TOKENS: usize = 16;
            let tokens: Vec<String> = query
                .split_whitespace()
                .map(|t| t.to_lowercase())
                .filter(|t| !t.is_empty())
                .take(MAX_TOKENS)
                .collect();
            for token in &tokens {
                // Escape LIKE wildcards in user input so `%` / `_` are matched
                // literally instead of acting as wildcards (avoids accidental
                // full-table scans and surprising matches).
                let pattern = format!("%{}%", escape_like_wildcards(token));
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push("(LOWER(ref) LIKE ");
                qb.push_bind(pattern.clone());
                qb.push(" ESCAPE '\\'");
                qb.push(" OR LOWER(label) LIKE ");
                qb.push_bind(pattern.clone());
                qb.push(" ESCAPE '\\'");
                qb.push(" OR LOWER(COALESCE(description, '')) LIKE ");
                qb.push_bind(pattern.clone());
                qb.push(" ESCAPE '\\'");
                qb.push(" OR LOWER(pack_ref) LIKE ");
                qb.push_bind(pattern.clone());
                qb.push(" ESCAPE '\\'");
                qb.push(")");

                count_qb.push("(LOWER(ref) LIKE ");
                count_qb.push_bind(pattern.clone());
                count_qb.push(" ESCAPE '\\'");
                count_qb.push(" OR LOWER(label) LIKE ");
                count_qb.push_bind(pattern.clone());
                count_qb.push(" ESCAPE '\\'");
                count_qb.push(" OR LOWER(COALESCE(description, '')) LIKE ");
                count_qb.push_bind(pattern.clone());
                count_qb.push(" ESCAPE '\\'");
                count_qb.push(" OR LOWER(pack_ref) LIKE ");
                count_qb.push_bind(pattern);
                count_qb.push(" ESCAPE '\\'");
                count_qb.push(")");
            }
        }

        // Suppress unused-assignment warning from the macro's last expansion.
        let _ = has_where;

        // Count
        let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;
        let total = total.max(0) as u64;

        // Data query
        qb.push(" ORDER BY ref ASC");
        qb.push(" LIMIT ");
        qb.push_bind(filters.limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let rows: Vec<Action> = qb.build_query_as().fetch_all(db).await?;

        Ok(ActionSearchResult { rows, total })
    }

    /// Find actions by pack ID
    pub async fn find_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Vec<Action>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let actions = sqlx::query_as::<_, Action>(&format!(
            "SELECT {} FROM action WHERE pack = $1 ORDER BY ref ASC",
            ACTION_COLUMNS
        ))
        .bind(pack_id)
        .fetch_all(executor)
        .await?;

        Ok(actions)
    }

    /// Find actions by runtime ID
    pub async fn find_by_runtime<'e, E>(executor: E, runtime_id: Id) -> Result<Vec<Action>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let actions = sqlx::query_as::<_, Action>(&format!(
            "SELECT {} FROM action WHERE runtime = $1 ORDER BY ref ASC",
            ACTION_COLUMNS
        ))
        .bind(runtime_id)
        .fetch_all(executor)
        .await?;

        Ok(actions)
    }

    /// Search actions by name/label
    pub async fn search<'e, E>(executor: E, query: &str) -> Result<Vec<Action>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let search_pattern = format!("%{}%", query.to_lowercase());
        let actions = sqlx::query_as::<_, Action>(&format!(
            "SELECT {} FROM action WHERE LOWER(ref) LIKE $1 OR LOWER(label) LIKE $1 OR LOWER(description) LIKE $1 ORDER BY ref ASC",
            ACTION_COLUMNS
        ))
        .bind(&search_pattern)
        .fetch_all(executor)
        .await?;

        Ok(actions)
    }

    /// Find all workflow actions (actions linked to a workflow definition)
    pub async fn find_workflows<'e, E>(executor: E) -> Result<Vec<Action>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let actions = sqlx::query_as::<_, Action>(&format!(
            "SELECT {} FROM action WHERE workflow_def IS NOT NULL ORDER BY ref ASC",
            ACTION_COLUMNS
        ))
        .fetch_all(executor)
        .await?;

        Ok(actions)
    }

    /// Find action by workflow definition ID
    pub async fn find_by_workflow_def<'e, E>(
        executor: E,
        workflow_def_id: Id,
    ) -> Result<Option<Action>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let action = sqlx::query_as::<_, Action>(&format!(
            "SELECT {} FROM action WHERE workflow_def = $1",
            ACTION_COLUMNS
        ))
        .bind(workflow_def_id)
        .fetch_optional(executor)
        .await?;

        Ok(action)
    }

    /// Delete non-adhoc actions belonging to a pack whose refs are NOT in the given set.
    ///
    /// Used during pack reinstallation to clean up actions that were removed
    /// from the pack's YAML files. Ad-hoc (user-created) actions are preserved.
    pub async fn delete_non_adhoc_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        keep_refs: &[String],
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = if keep_refs.is_empty() {
            sqlx::query("DELETE FROM action WHERE pack = $1 AND is_adhoc = false")
                .bind(pack_id)
                .execute(executor)
                .await?
        } else {
            sqlx::query(
                "DELETE FROM action WHERE pack = $1 AND is_adhoc = false AND ref != ALL($2)",
            )
            .bind(pack_id)
            .bind(keep_refs)
            .execute(executor)
            .await?
        };

        Ok(result.rows_affected())
    }

    /// Link an action to a workflow definition
    pub async fn link_workflow_def<'e, E>(
        executor: E,
        action_id: Id,
        workflow_def_id: Id,
    ) -> Result<Action>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let action = sqlx::query_as::<_, Action>(&format!(
            r#"
            UPDATE action
            SET workflow_def = $2, updated = NOW()
            WHERE id = $1
            RETURNING {}
            "#,
            ACTION_COLUMNS
        ))
        .bind(action_id)
        .bind(workflow_def_id)
        .fetch_one(executor)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => Error::not_found("action", "id", action_id.to_string()),
            _ => e.into(),
        })?;

        Ok(action)
    }
}

// ============================================================================
// Policy Repository
// ============================================================================

/// Repository for Policy operations
pub struct PolicyRepository;

impl Repository for PolicyRepository {
    type Entity = Policy;

    fn table_name() -> &'static str {
        "policy"
    }
}

/// Input for creating a new policy
#[derive(Debug, Clone)]
pub struct CreatePolicyInput {
    pub r#ref: String,
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub action: Option<Id>,
    pub action_ref: Option<String>,
    pub parameters: Vec<String>,
    pub method: PolicyMethod,
    pub threshold: i32,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

/// Input for updating a policy
#[derive(Debug, Clone, Default)]
pub struct UpdatePolicyInput {
    pub parameters: Option<Vec<String>>,
    pub method: Option<PolicyMethod>,
    pub threshold: Option<i32>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[async_trait::async_trait]
impl FindById for PolicyRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let policy = sqlx::query_as::<_, Policy>(
            r#"
            SELECT id, ref, pack, pack_ref, action, action_ref, parameters, method,
                   threshold, name, description, tags, created, updated
            FROM policy
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(policy)
    }
}

#[async_trait::async_trait]
impl FindByRef for PolicyRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let policy = sqlx::query_as::<_, Policy>(
            r#"
            SELECT id, ref, pack, pack_ref, action, action_ref, parameters, method,
                   threshold, name, description, tags, created, updated
            FROM policy
            WHERE ref = $1
            "#,
        )
        .bind(ref_str)
        .fetch_optional(executor)
        .await?;

        Ok(policy)
    }
}

#[async_trait::async_trait]
impl List for PolicyRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let policies = sqlx::query_as::<_, Policy>(
            r#"
            SELECT id, ref, pack, pack_ref, action, action_ref, parameters, method,
                   threshold, name, description, tags, created, updated
            FROM policy
            ORDER BY ref ASC
            "#,
        )
        .fetch_all(executor)
        .await?;

        Ok(policies)
    }
}

#[async_trait::async_trait]
impl Create for PolicyRepository {
    type CreateInput = CreatePolicyInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Try to insert - database will enforce uniqueness constraint
        let policy = sqlx::query_as::<_, Policy>(
            r#"
            INSERT INTO policy (ref, pack, pack_ref, action, action_ref, parameters,
                                 method, threshold, name, description, tags)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id, ref, pack, pack_ref, action, action_ref, parameters, method,
                      threshold, name, description, tags, created, updated
            "#,
        )
        .bind(&input.r#ref)
        .bind(input.pack)
        .bind(&input.pack_ref)
        .bind(input.action)
        .bind(&input.action_ref)
        .bind(&input.parameters)
        .bind(input.method)
        .bind(input.threshold)
        .bind(&input.name)
        .bind(&input.description)
        .bind(&input.tags)
        .fetch_one(executor)
        .await
        .map_err(|e| {
            // Convert unique constraint violation to AlreadyExists error
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.is_unique_violation() {
                    return Error::already_exists("Policy", "ref", &input.r#ref);
                }
            }
            e.into()
        })?;

        Ok(policy)
    }
}

#[async_trait::async_trait]
impl Update for PolicyRepository {
    type UpdateInput = UpdatePolicyInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query = QueryBuilder::new("UPDATE policy SET ");
        let mut has_updates = false;

        if let Some(parameters) = &input.parameters {
            if has_updates {
                query.push(", ");
            }
            query.push("parameters = ");
            query.push_bind(parameters);
            has_updates = true;
        }

        if let Some(method) = input.method {
            if has_updates {
                query.push(", ");
            }
            query.push("method = ");
            query.push_bind(method);
            has_updates = true;
        }

        if let Some(threshold) = input.threshold {
            if has_updates {
                query.push(", ");
            }
            query.push("threshold = ");
            query.push_bind(threshold);
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

        if let Some(description) = &input.description {
            if has_updates {
                query.push(", ");
            }
            query.push("description = ");
            query.push_bind(description);
            has_updates = true;
        }

        if let Some(tags) = &input.tags {
            if has_updates {
                query.push(", ");
            }
            query.push("tags = ");
            query.push_bind(tags);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing policy
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ");
        query.push_bind(id);
        query.push(" RETURNING id, ref, pack, pack_ref, action, action_ref, parameters, method, threshold, name, description, tags, created, updated");

        let policy = query.build_query_as::<Policy>().fetch_one(executor).await?;

        Ok(policy)
    }
}

#[async_trait::async_trait]
impl Delete for PolicyRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM policy WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl PolicyRepository {
    /// Find policies by action ID
    pub async fn find_by_action<'e, E>(executor: E, action_id: Id) -> Result<Vec<Policy>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let policies = sqlx::query_as::<_, Policy>(
            r#"
            SELECT id, ref, pack, pack_ref, action, action_ref, parameters, method,
                   threshold, name, description, tags, created, updated
            FROM policy
            WHERE action = $1
            ORDER BY ref ASC
            "#,
        )
        .bind(action_id)
        .fetch_all(executor)
        .await?;

        Ok(policies)
    }

    /// Find policies by tag
    pub async fn find_by_tag<'e, E>(executor: E, tag: &str) -> Result<Vec<Policy>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let policies = sqlx::query_as::<_, Policy>(
            r#"
            SELECT id, ref, pack, pack_ref, action, action_ref, parameters, method,
                   threshold, name, description, tags, created, updated
            FROM policy
            WHERE $1 = ANY(tags)
            ORDER BY ref ASC
            "#,
        )
        .bind(tag)
        .fetch_all(executor)
        .await?;

        Ok(policies)
    }

    /// Find the most recent action-specific policy.
    pub async fn find_latest_by_action<'e, E>(executor: E, action_id: Id) -> Result<Option<Policy>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let policy = sqlx::query_as::<_, Policy>(
            r#"
            SELECT id, ref, pack, pack_ref, action, action_ref, parameters, method,
                   threshold, name, description, tags, created, updated
            FROM policy
            WHERE action = $1
            ORDER BY created DESC
            LIMIT 1
            "#,
        )
        .bind(action_id)
        .fetch_optional(executor)
        .await?;

        Ok(policy)
    }

    /// Find the most recent pack-specific policy.
    pub async fn find_latest_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Option<Policy>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let policy = sqlx::query_as::<_, Policy>(
            r#"
            SELECT id, ref, pack, pack_ref, action, action_ref, parameters, method,
                   threshold, name, description, tags, created, updated
            FROM policy
            WHERE pack = $1 AND action IS NULL
            ORDER BY created DESC
            LIMIT 1
            "#,
        )
        .bind(pack_id)
        .fetch_optional(executor)
        .await?;

        Ok(policy)
    }

    /// Find the most recent global policy.
    pub async fn find_latest_global<'e, E>(executor: E) -> Result<Option<Policy>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let policy = sqlx::query_as::<_, Policy>(
            r#"
            SELECT id, ref, pack, pack_ref, action, action_ref, parameters, method,
                   threshold, name, description, tags, created, updated
            FROM policy
            WHERE pack IS NULL AND action IS NULL
            ORDER BY created DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(executor)
        .await?;

        Ok(policy)
    }
}
