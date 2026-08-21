//! Action and Policy repository for database operations
//!
//! This module provides CRUD operations and queries for Action and Policy entities.

use crate::models::{
    action::*, enums::ActionReferenceVisibility, enums::PolicyMethod, Id, JsonDict, JsonSchema,
    RetentionPolicyType,
};
use crate::scheduling::{parse_worker_affinity, parse_worker_selector, parse_worker_tolerations};
use crate::version_matching::parse_constraint;
use crate::{Error, Result};
use serde_json::Value as JsonValue;
use sqlx::{Executor, Postgres, QueryBuilder};

use super::{
    text_search_patterns, Create, Delete, FindById, FindByRef, List, Patch, Repository, Update,
};

/// Columns selected in all Action queries. Must match the `Action` model's `FromRow` fields.
pub const ACTION_COLUMNS: &str = "id, ref, pack, pack_ref, label, description, entrypoint, \
    runtime, enabled, runtime_version_constraint, required_worker_runtimes, \
    worker_selector, worker_tolerations, worker_affinity, \
    param_schema, out_schema, workflow_def, is_adhoc, accesses_mcp, \
    default_execution_permission_set_refs, \
    reference_visibility, reference_allowed_pack_refs, \
    log_retention_policy, log_retention_limit, artifact_retention_policy, artifact_retention_limit, \
    timeout_seconds, \
    parameter_delivery, parameter_format, output_format, created, updated";

/// Columns selected in all Policy queries. Must match the `Policy` model's `FromRow` fields.
pub const POLICY_COLUMNS: &str = "id, ref, pack, pack_ref, action, action_ref, enabled, priority, \
    parameters, method, threshold, rate_limit_max_executions, rate_limit_window_seconds, quotas, \
    name, description, tags, created, updated";

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

pub fn validate_action_reference_visibility_config(
    visibility: ActionReferenceVisibility,
    allowed_pack_refs: &[String],
) -> Result<()> {
    for pack_ref in allowed_pack_refs {
        crate::schema::RefValidator::validate_pack_ref(pack_ref)?;
    }

    if visibility != ActionReferenceVisibility::Restricted && !allowed_pack_refs.is_empty() {
        return Err(Error::validation(
            "reference_allowed_pack_refs may only be set when reference_visibility is restricted",
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
    pub reference_visibility: ActionReferenceVisibility,
    pub reference_allowed_pack_refs: Vec<String>,
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
    pub reference_visibility: Option<ActionReferenceVisibility>,
    pub reference_allowed_pack_refs: Option<Vec<String>>,
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
        validate_action_reference_visibility_config(
            input.reference_visibility,
            &input.reference_allowed_pack_refs,
        )?;
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
                                  reference_visibility, reference_allowed_pack_refs,
                                  log_retention_policy, log_retention_limit,
                                  artifact_retention_policy, artifact_retention_limit, timeout_seconds)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25)
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
        .bind(input.reference_visibility)
        .bind(&input.reference_allowed_pack_refs)
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
        if let Some(visibility) = input.reference_visibility {
            if visibility != ActionReferenceVisibility::Restricted {
                if let Some(allowed_pack_refs) = &input.reference_allowed_pack_refs {
                    if !allowed_pack_refs.is_empty() {
                        return Err(Error::validation(
                            "reference_allowed_pack_refs may only be set when reference_visibility is restricted",
                        ));
                    }
                }
            }
        }
        if let Some(allowed_pack_refs) = &input.reference_allowed_pack_refs {
            for pack_ref in allowed_pack_refs {
                crate::schema::RefValidator::validate_pack_ref(pack_ref)?;
            }
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

        if let Some(reference_visibility) = input.reference_visibility {
            if has_updates {
                query.push(", ");
            }
            query.push("reference_visibility = ");
            query.push_bind(reference_visibility);
            has_updates = true;
        }

        if let Some(reference_allowed_pack_refs) = &input.reference_allowed_pack_refs {
            if has_updates {
                query.push(", ");
            }
            query.push("reference_allowed_pack_refs = ");
            query.push_bind(reference_allowed_pack_refs);
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
        if filters.query.is_some() {
            // Tokenize the query: each whitespace-separated token must match
            // at least one of (ref, label, description, pack_ref). This lets
            // callers find an action with multi-keyword searches like
            // "slack post message" without caring about field ordering.
            //
            // Cap token count to bound query cost and prevent pathological
            // inputs from generating huge OR/AND trees.
            for pattern in text_search_patterns(filters.query.as_deref()) {
                // Escape LIKE wildcards in user input so `%` / `_` are matched
                // literally instead of acting as wildcards (avoids accidental
                // full-table scans and surprising matches).
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

    /// Count actions belonging to a pack by its ref
    pub async fn count_by_pack_ref<'e, E>(executor: E, pack_ref: &str) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM action WHERE pack_ref = $1")
            .bind(pack_ref)
            .fetch_one(executor)
            .await?;
        Ok(result.0)
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
    pub enabled: bool,
    pub priority: i32,
    pub parameters: Vec<String>,
    pub method: Option<PolicyMethod>,
    pub threshold: Option<i32>,
    pub rate_limit_max_executions: Option<i32>,
    pub rate_limit_window_seconds: Option<i32>,
    pub quotas: JsonValue,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

/// Input for updating a policy
#[derive(Debug, Clone, Default)]
pub struct UpdatePolicyInput {
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub parameters: Option<Vec<String>>,
    pub method: Option<Option<PolicyMethod>>,
    pub threshold: Option<Option<i32>>,
    pub rate_limit_max_executions: Option<Option<i32>>,
    pub rate_limit_window_seconds: Option<Option<i32>>,
    pub quotas: Option<JsonValue>,
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
}

/// Filters for [`PolicyRepository::list_search`].
#[derive(Debug, Clone, Default)]
pub struct PolicySearchFilters {
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub action: Option<Id>,
    pub action_ref: Option<String>,
    pub scope: Option<PolicyScopeFilter>,
    pub enabled: Option<bool>,
    pub tag: Option<String>,
    /// SQL-side RBAC visibility filter. `None` means "no visibility restriction"
    /// (e.g. token types that bypass RBAC entirely). `Some` with an empty scope
    /// list means "nothing is visible" (deny-all).
    pub visibility: Option<PolicyVisibilityFilter>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyScopeFilter {
    Global,
    Pack,
    Action,
}

/// One OR-branch of policy visibility, derived from a single RBAC grant.
///
/// Each `Some` field is an AND-ed condition; a scope with all fields `None`
/// matches every row (i.e. an unconstrained grant).
#[derive(Debug, Clone, Default)]
pub struct PolicyVisibilityScope {
    /// Allowed pack refs. A policy matches if its own `pack_ref` is in this
    /// list, or (when `pack_ref` is null) the pack portion of its
    /// `action_ref` is in this list -- mirroring how `AuthorizationContext`
    /// derives `pack_ref` from `action_ref` for action-scoped policies.
    pub pack_refs: Option<Vec<String>>,
    /// Allowed action refs (matches `policy.action_ref`).
    pub action_refs: Option<Vec<String>>,
    /// Allowed policy refs (matches `policy.ref`).
    pub refs: Option<Vec<String>>,
    /// Allowed policy IDs (matches `policy.id`).
    pub ids: Option<Vec<Id>>,
}

#[derive(Debug, Clone, Default)]
pub struct PolicyVisibilityFilter {
    pub scopes: Vec<PolicyVisibilityScope>,
}

#[derive(Debug, Clone)]
pub struct PolicySearchResult {
    pub rows: Vec<Policy>,
    pub total: u64,
}

fn push_policy_filters<'args>(
    query: &mut QueryBuilder<'args, Postgres>,
    filters: &'args PolicySearchFilters,
) {
    if let Some(pack) = filters.pack {
        query.push(" AND pack = ");
        query.push_bind(pack);
    }
    if let Some(pack_ref) = &filters.pack_ref {
        query.push(" AND pack_ref = ");
        query.push_bind(pack_ref);
    }
    if let Some(action) = filters.action {
        query.push(" AND action = ");
        query.push_bind(action);
    }
    if let Some(action_ref) = &filters.action_ref {
        query.push(" AND action_ref = ");
        query.push_bind(action_ref);
    }
    if let Some(enabled) = filters.enabled {
        query.push(" AND enabled = ");
        query.push_bind(enabled);
    }
    if let Some(tag) = &filters.tag {
        query.push(" AND ");
        query.push_bind(tag);
        query.push(" = ANY(tags)");
    }
    match filters.scope {
        Some(PolicyScopeFilter::Global) => {
            query.push(" AND pack IS NULL AND action IS NULL");
        }
        Some(PolicyScopeFilter::Pack) => {
            query.push(" AND pack IS NOT NULL AND action IS NULL");
        }
        Some(PolicyScopeFilter::Action) => {
            query.push(" AND action IS NOT NULL");
        }
        None => {}
    }

    push_policy_visibility_filter(query, filters.visibility.as_ref());
}

/// Appends a SQL-side RBAC visibility predicate built from `visibility`.
///
/// `None` applies no restriction. `Some` with an empty scope list is a
/// deny-all (no grant matched). Otherwise each scope becomes an OR-ed,
/// AND-of-its-present-fields condition, mirroring per-row `Grant::allows`
/// evaluation for the `Resource::Policies` context.
fn push_policy_visibility_filter<'args>(
    query: &mut QueryBuilder<'args, Postgres>,
    visibility: Option<&'args PolicyVisibilityFilter>,
) {
    let Some(visibility) = visibility else {
        return;
    };

    if visibility.scopes.is_empty() {
        query.push(" AND FALSE");
        return;
    }

    query.push(" AND (");
    for (index, scope) in visibility.scopes.iter().enumerate() {
        if index > 0 {
            query.push(" OR ");
        }
        query.push("(");
        let mut wrote = false;

        if let Some(pack_refs) = &scope.pack_refs {
            query.push("(pack_ref = ANY(");
            query.push_bind(pack_refs);
            query.push(") OR (pack_ref IS NULL AND split_part(action_ref, '.', 1) = ANY(");
            query.push_bind(pack_refs);
            query.push(")))");
            wrote = true;
        }
        if let Some(action_refs) = &scope.action_refs {
            if wrote {
                query.push(" AND ");
            }
            query.push("action_ref = ANY(");
            query.push_bind(action_refs);
            query.push(")");
            wrote = true;
        }
        if let Some(refs) = &scope.refs {
            if wrote {
                query.push(" AND ");
            }
            query.push("ref = ANY(");
            query.push_bind(refs);
            query.push(")");
            wrote = true;
        }
        if let Some(ids) = &scope.ids {
            if wrote {
                query.push(" AND ");
            }
            query.push("id = ANY(");
            query.push_bind(ids);
            query.push(")");
            wrote = true;
        }

        if !wrote {
            query.push("TRUE");
        }
        query.push(")");
    }
    query.push(")");
}

#[async_trait::async_trait]
impl FindById for PolicyRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM policy WHERE id = $1", POLICY_COLUMNS);
        let policy = sqlx::query_as::<_, Policy>(&query)
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
        let query = format!("SELECT {} FROM policy WHERE ref = $1", POLICY_COLUMNS);
        let policy = sqlx::query_as::<_, Policy>(&query)
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
        let query = format!("SELECT {} FROM policy ORDER BY ref ASC", POLICY_COLUMNS);
        let policies = sqlx::query_as::<_, Policy>(&query)
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
        let policy = sqlx::query_as::<_, Policy>(&format!(
            r#"
            INSERT INTO policy (ref, pack, pack_ref, action, action_ref, enabled, priority,
                                 parameters, method, threshold, rate_limit_max_executions,
                                 rate_limit_window_seconds, quotas, name, description, tags)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            RETURNING {}
            "#,
            POLICY_COLUMNS
        ))
        .bind(&input.r#ref)
        .bind(input.pack)
        .bind(&input.pack_ref)
        .bind(input.action)
        .bind(&input.action_ref)
        .bind(input.enabled)
        .bind(input.priority)
        .bind(&input.parameters)
        .bind(input.method)
        .bind(input.threshold)
        .bind(input.rate_limit_max_executions)
        .bind(input.rate_limit_window_seconds)
        .bind(&input.quotas)
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

        if let Some(enabled) = input.enabled {
            if has_updates {
                query.push(", ");
            }
            query.push("enabled = ");
            query.push_bind(enabled);
            has_updates = true;
        }

        if let Some(priority) = input.priority {
            if has_updates {
                query.push(", ");
            }
            query.push("priority = ");
            query.push_bind(priority);
            has_updates = true;
        }

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

        if let Some(rate_limit_max_executions) = input.rate_limit_max_executions {
            if has_updates {
                query.push(", ");
            }
            query.push("rate_limit_max_executions = ");
            query.push_bind(rate_limit_max_executions);
            has_updates = true;
        }

        if let Some(rate_limit_window_seconds) = input.rate_limit_window_seconds {
            if has_updates {
                query.push(", ");
            }
            query.push("rate_limit_window_seconds = ");
            query.push_bind(rate_limit_window_seconds);
            has_updates = true;
        }

        if let Some(quotas) = &input.quotas {
            if has_updates {
                query.push(", ");
            }
            query.push("quotas = ");
            query.push_bind(quotas);
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
        query.push(" RETURNING ");
        query.push(POLICY_COLUMNS);

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
    pub async fn list_search<'e, E>(
        executor: E,
        filters: &PolicySearchFilters,
    ) -> Result<PolicySearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let limit = if filters.limit <= 0 {
            50
        } else {
            filters.limit
        };
        let offset = filters.offset.max(0);

        let mut query = QueryBuilder::new("SELECT ");
        query.push(POLICY_COLUMNS);
        query.push(" FROM policy WHERE 1=1");
        push_policy_filters(&mut query, filters);
        query.push(" ORDER BY priority DESC, ref ASC LIMIT ");
        query.push_bind(limit);
        query.push(" OFFSET ");
        query.push_bind(offset);

        let rows = query.build_query_as::<Policy>().fetch_all(executor).await?;

        let mut count_query = QueryBuilder::new("SELECT COUNT(*) FROM policy WHERE 1=1");
        push_policy_filters(&mut count_query, filters);
        let total: i64 = count_query.build_query_scalar().fetch_one(executor).await?;

        Ok(PolicySearchResult {
            rows,
            total: total as u64,
        })
    }

    /// Find policies by action ID
    pub async fn find_by_action<'e, E>(executor: E, action_id: Id) -> Result<Vec<Policy>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM policy WHERE action = $1 ORDER BY ref ASC",
            POLICY_COLUMNS
        );
        let policies = sqlx::query_as::<_, Policy>(&query)
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
        let query = format!(
            "SELECT {} FROM policy WHERE $1 = ANY(tags) ORDER BY ref ASC",
            POLICY_COLUMNS
        );
        let policies = sqlx::query_as::<_, Policy>(&query)
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
        let query = format!(
            "SELECT {} FROM policy WHERE enabled = true AND action = $1 ORDER BY priority DESC, created DESC LIMIT 1",
            POLICY_COLUMNS
        );
        let policy = sqlx::query_as::<_, Policy>(&query)
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
        let query = format!(
            "SELECT {} FROM policy WHERE enabled = true AND pack = $1 AND action IS NULL ORDER BY priority DESC, created DESC LIMIT 1",
            POLICY_COLUMNS
        );
        let policy = sqlx::query_as::<_, Policy>(&query)
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
        let query = format!(
            "SELECT {} FROM policy WHERE enabled = true AND pack IS NULL AND action IS NULL ORDER BY priority DESC, created DESC LIMIT 1",
            POLICY_COLUMNS
        );
        let policy = sqlx::query_as::<_, Policy>(&query)
            .fetch_optional(executor)
            .await?;

        Ok(policy)
    }

    /// Delete pack-owned policies whose refs are no longer present in pack files.
    pub async fn delete_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        keep_refs: &[String],
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            r#"
            DELETE FROM policy
            WHERE pack = $1
              AND NOT (ref = ANY($2))
            "#,
        )
        .bind(pack_id)
        .bind(keep_refs)
        .execute(executor)
        .await?;

        Ok(result.rows_affected())
    }
}
