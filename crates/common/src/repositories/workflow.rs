//! Workflow repository for database operations

use crate::models::{enums::ExecutionStatus, workflow::*, Id, JsonDict, JsonSchema};
use crate::Result;
use sqlx::{Executor, PgConnection, PgPool, Postgres, QueryBuilder};

use super::{Create, Delete, FindById, FindByRef, List, Repository, Update};

const WORKFLOW_DEFINITION_COLUMNS: &str = "id, ref, pack, pack_ref, label, description, version, param_schema, out_schema, definition, tags, retired_at, created, updated";

// ============================================================================
// Workflow Definition Search
// ============================================================================

/// Filters for [`WorkflowDefinitionRepository::list_search`].
///
/// All fields are optional and combinable (AND). Pagination is always applied.
/// Tag filtering uses `ANY(tags)` for each tag (OR across tags, AND with other filters).
#[derive(Debug, Clone, Default)]
pub struct WorkflowSearchFilters {
    /// Filter by pack ID
    pub pack: Option<Id>,
    /// Filter by pack reference
    pub pack_ref: Option<String>,
    /// Filter by tags (OR across tags — matches if any tag is present)
    pub tags: Option<Vec<String>>,
    /// Text search across label and description (case-insensitive substring)
    pub search: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

/// Result of [`WorkflowDefinitionRepository::list_search`].
#[derive(Debug)]
pub struct WorkflowSearchResult {
    pub rows: Vec<WorkflowDefinition>,
    pub total: u64,
}

// ============================================================================
// WORKFLOW DEFINITION REPOSITORY
// ============================================================================

pub struct WorkflowDefinitionRepository;

impl Repository for WorkflowDefinitionRepository {
    type Entity = WorkflowDefinition;
    fn table_name() -> &'static str {
        "workflow_definition"
    }
}

#[derive(Debug, Clone)]
pub struct CreateWorkflowDefinitionInput {
    pub r#ref: String,
    pub pack: Id,
    pub pack_ref: String,
    pub label: String,
    pub description: Option<String>,
    pub version: String,
    pub param_schema: Option<JsonSchema>,
    pub out_schema: Option<JsonSchema>,
    pub definition: JsonDict,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateWorkflowDefinitionInput {
    pub label: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub param_schema: Option<JsonSchema>,
    pub out_schema: Option<JsonSchema>,
    pub definition: Option<JsonDict>,
    pub tags: Option<Vec<String>>,
}

#[async_trait::async_trait]
impl FindById for WorkflowDefinitionRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "SELECT {WORKFLOW_DEFINITION_COLUMNS} FROM workflow_definition WHERE id = $1 AND retired_at IS NULL"
        ))
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl FindByRef for WorkflowDefinitionRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "SELECT {WORKFLOW_DEFINITION_COLUMNS} FROM workflow_definition WHERE ref = $1 AND retired_at IS NULL"
        ))
        .bind(ref_str)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for WorkflowDefinitionRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "SELECT {WORKFLOW_DEFINITION_COLUMNS} FROM workflow_definition WHERE retired_at IS NULL ORDER BY created DESC LIMIT 1000"
        ))
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for WorkflowDefinitionRepository {
    type CreateInput = CreateWorkflowDefinitionInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "INSERT INTO workflow_definition
             (ref, pack, pack_ref, label, description, version, param_schema, out_schema, definition, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING {WORKFLOW_DEFINITION_COLUMNS}"
        ))
        .bind(&input.r#ref)
        .bind(input.pack)
        .bind(&input.pack_ref)
        .bind(&input.label)
        .bind(&input.description)
        .bind(&input.version)
        .bind(&input.param_schema)
        .bind(&input.out_schema)
        .bind(&input.definition)
        .bind(&input.tags)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for WorkflowDefinitionRepository {
    type UpdateInput = UpdateWorkflowDefinitionInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query = QueryBuilder::new("UPDATE workflow_definition SET ");
        let mut has_updates = false;

        if let Some(label) = &input.label {
            query.push("label = ").push_bind(label);
            has_updates = true;
        }
        if let Some(description) = &input.description {
            if has_updates {
                query.push(", ");
            }
            query.push("description = ").push_bind(description);
            has_updates = true;
        }
        if let Some(version) = &input.version {
            if has_updates {
                query.push(", ");
            }
            query.push("version = ").push_bind(version);
            has_updates = true;
        }
        if let Some(param_schema) = &input.param_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("param_schema = ").push_bind(param_schema);
            has_updates = true;
        }
        if let Some(out_schema) = &input.out_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("out_schema = ").push_bind(out_schema);
            has_updates = true;
        }
        if let Some(definition) = &input.definition {
            if has_updates {
                query.push(", ");
            }
            query.push("definition = ").push_bind(definition);
            has_updates = true;
        }
        if let Some(tags) = &input.tags {
            if has_updates {
                query.push(", ");
            }
            query.push("tags = ").push_bind(tags);
            has_updates = true;
        }
        if !has_updates {
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(" RETURNING ").push(WORKFLOW_DEFINITION_COLUMNS);

        query
            .build_query_as::<WorkflowDefinition>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for WorkflowDefinitionRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM workflow_definition WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl WorkflowDefinitionRepository {
    pub async fn find_by_id_including_retired<'e, E>(
        executor: E,
        id: Id,
    ) -> Result<Option<WorkflowDefinition>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "SELECT {WORKFLOW_DEFINITION_COLUMNS} FROM workflow_definition WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_ref_including_retired<'e, E>(
        executor: E,
        ref_str: &str,
    ) -> Result<Option<WorkflowDefinition>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "SELECT {WORKFLOW_DEFINITION_COLUMNS} FROM workflow_definition WHERE ref = $1"
        ))
        .bind(ref_str)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    /// Search workflow definitions with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    /// Tags use an OR match — a workflow matches if it contains ANY of the
    /// requested tags (via `tags && ARRAY[...]`).
    pub async fn list_search<'e, E>(
        db: E,
        filters: &WorkflowSearchFilters,
    ) -> Result<WorkflowSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let select_cols = WORKFLOW_DEFINITION_COLUMNS;

        let mut qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(format!(
            "SELECT {select_cols} FROM workflow_definition WHERE retired_at IS NULL"
        ));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM workflow_definition WHERE retired_at IS NULL");

        let mut has_where = true;

        macro_rules! push_condition {
            ($cond_prefix:expr, $value:expr) => {{
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push($cond_prefix);
                qb.push_bind($value.clone());
                count_qb.push($cond_prefix);
                count_qb.push_bind($value);
            }};
        }

        if let Some(pack_id) = filters.pack {
            push_condition!("pack = ", pack_id);
        }
        if let Some(ref pack_ref) = filters.pack_ref {
            push_condition!("pack_ref = ", pack_ref.clone());
        }
        if let Some(ref tags) = filters.tags {
            if !tags.is_empty() {
                // Use PostgreSQL array overlap operator: tags && ARRAY[...]
                push_condition!("tags && ", tags.clone());
            }
        }
        if let Some(ref search) = filters.search {
            let pattern = format!("%{}%", search.to_lowercase());
            // Search needs an OR across multiple columns, wrapped in parens
            if !has_where {
                qb.push(" WHERE ");
                count_qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
                count_qb.push(" AND ");
            }
            qb.push("(LOWER(label) LIKE ");
            qb.push_bind(pattern.clone());
            qb.push(" OR LOWER(COALESCE(description, '')) LIKE ");
            qb.push_bind(pattern.clone());
            qb.push(")");

            count_qb.push("(LOWER(label) LIKE ");
            count_qb.push_bind(pattern.clone());
            count_qb.push(" OR LOWER(COALESCE(description, '')) LIKE ");
            count_qb.push_bind(pattern);
            count_qb.push(")");
        }

        // Suppress unused-assignment warning from the macro's last expansion.
        let _ = has_where;

        // Count
        let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;
        let total = total.max(0) as u64;

        // Data query
        qb.push(" ORDER BY label ASC");
        qb.push(" LIMIT ");
        qb.push_bind(filters.limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let rows: Vec<WorkflowDefinition> = qb.build_query_as().fetch_all(db).await?;

        Ok(WorkflowSearchResult { rows, total })
    }

    /// Find all workflows for a specific pack by pack ID
    pub async fn find_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Vec<WorkflowDefinition>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "SELECT {WORKFLOW_DEFINITION_COLUMNS} FROM workflow_definition \
             WHERE pack = $1 AND retired_at IS NULL ORDER BY label"
        ))
        .bind(pack_id)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    /// Find all workflows for a specific pack by pack reference
    pub async fn find_by_pack_ref<'e, E>(
        executor: E,
        pack_ref: &str,
    ) -> Result<Vec<WorkflowDefinition>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "SELECT {WORKFLOW_DEFINITION_COLUMNS} FROM workflow_definition \
             WHERE pack_ref = $1 AND retired_at IS NULL ORDER BY label"
        ))
        .bind(pack_ref)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    /// Count workflows for a specific pack by pack reference
    pub async fn count_by_pack<'e, E>(executor: E, pack_ref: &str) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM workflow_definition WHERE pack_ref = $1 AND retired_at IS NULL",
        )
        .bind(pack_ref)
        .fetch_one(executor)
        .await?;
        Ok(result.0)
    }

    /// Find workflows by tag
    pub async fn find_by_tag<'e, E>(executor: E, tag: &str) -> Result<Vec<WorkflowDefinition>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowDefinition>(&format!(
            "SELECT {WORKFLOW_DEFINITION_COLUMNS} FROM workflow_definition \
             WHERE $1 = ANY(tags) AND retired_at IS NULL ORDER BY label"
        ))
        .bind(tag)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }
}

// ============================================================================
// WORKFLOW EXECUTION REPOSITORY
// ============================================================================

pub struct WorkflowExecutionRepository;

#[derive(Debug, Clone)]
pub struct WorkflowExecutionCreateOrGetResult {
    pub workflow_execution: WorkflowExecution,
    pub created: bool,
}

impl Repository for WorkflowExecutionRepository {
    type Entity = WorkflowExecution;
    fn table_name() -> &'static str {
        "workflow_execution"
    }
}

#[derive(Debug, Clone)]
pub struct CreateWorkflowExecutionInput {
    pub execution: Id,
    pub workflow_def: Id,
    pub task_graph: JsonDict,
    pub variables: JsonDict,
    pub status: ExecutionStatus,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateWorkflowExecutionInput {
    pub current_tasks: Option<Vec<String>>,
    pub completed_tasks: Option<Vec<String>>,
    pub failed_tasks: Option<Vec<String>>,
    pub skipped_tasks: Option<Vec<String>>,
    pub variables: Option<JsonDict>,
    pub status: Option<ExecutionStatus>,
    pub error_message: Option<String>,
    pub paused: Option<bool>,
    pub pause_reason: Option<String>,
}

#[async_trait::async_trait]
impl FindById for WorkflowExecutionRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowExecution>(
            "SELECT id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                    variables, task_graph, status, error_message, paused, pause_reason, created, updated
             FROM workflow_execution
             WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for WorkflowExecutionRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowExecution>(
            "SELECT id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                    variables, task_graph, status, error_message, paused, pause_reason, created, updated
             FROM workflow_execution
             ORDER BY created DESC
             LIMIT 1000"
        )
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for WorkflowExecutionRepository {
    type CreateInput = CreateWorkflowExecutionInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowExecution>(
            "INSERT INTO workflow_execution
             (execution, workflow_def, task_graph, variables, status)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                       variables, task_graph, status, error_message, paused, pause_reason, created, updated"
        )
        .bind(input.execution)
        .bind(input.workflow_def)
        .bind(&input.task_graph)
        .bind(&input.variables)
        .bind(input.status)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for WorkflowExecutionRepository {
    type UpdateInput = UpdateWorkflowExecutionInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query = QueryBuilder::new("UPDATE workflow_execution SET ");
        let mut has_updates = false;

        if let Some(current_tasks) = &input.current_tasks {
            query.push("current_tasks = ").push_bind(current_tasks);
            has_updates = true;
        }
        if let Some(completed_tasks) = &input.completed_tasks {
            if has_updates {
                query.push(", ");
            }
            query.push("completed_tasks = ").push_bind(completed_tasks);
            has_updates = true;
        }
        if let Some(failed_tasks) = &input.failed_tasks {
            if has_updates {
                query.push(", ");
            }
            query.push("failed_tasks = ").push_bind(failed_tasks);
            has_updates = true;
        }
        if let Some(skipped_tasks) = &input.skipped_tasks {
            if has_updates {
                query.push(", ");
            }
            query.push("skipped_tasks = ").push_bind(skipped_tasks);
            has_updates = true;
        }
        if let Some(variables) = &input.variables {
            if has_updates {
                query.push(", ");
            }
            query.push("variables = ").push_bind(variables);
            has_updates = true;
        }
        if let Some(status) = input.status {
            if has_updates {
                query.push(", ");
            }
            query.push("status = ").push_bind(status);
            has_updates = true;
        }
        if let Some(error_message) = &input.error_message {
            if has_updates {
                query.push(", ");
            }
            query.push("error_message = ").push_bind(error_message);
            has_updates = true;
        }
        if let Some(paused) = input.paused {
            if has_updates {
                query.push(", ");
            }
            query.push("paused = ").push_bind(paused);
            has_updates = true;
        }
        if let Some(pause_reason) = &input.pause_reason {
            if has_updates {
                query.push(", ");
            }
            query.push("pause_reason = ").push_bind(pause_reason);
            has_updates = true;
        }

        if !has_updates {
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(" RETURNING id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks, variables, task_graph, status, error_message, paused, pause_reason, created, updated");

        query
            .build_query_as::<WorkflowExecution>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for WorkflowExecutionRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM workflow_execution WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl WorkflowExecutionRepository {
    pub async fn acquire_advisory_lock(conn: &mut PgConnection, id: Id) -> Result<()> {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(id)
            .execute(conn)
            .await?;
        Ok(())
    }

    pub async fn cancel_with_prerequisites(
        pool: &PgPool,
        id: Id,
        error_message: &str,
        parent_update: Option<(ExecutionStatus, Option<serde_json::Value>)>,
    ) -> Result<Option<WorkflowExecution>> {
        let mut transaction = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        let updated = Self::cancel_with_prerequisites_with_conn(
            &mut transaction,
            id,
            error_message,
            parent_update,
        )
        .await?;
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn cancel_with_prerequisites_with_conn(
        conn: &mut PgConnection,
        id: Id,
        error_message: &str,
        parent_update: Option<(ExecutionStatus, Option<serde_json::Value>)>,
    ) -> Result<Option<WorkflowExecution>> {
        let Some(workflow) = Self::find_by_id_for_update(&mut *conn, id).await? else {
            return Ok(None);
        };
        if workflow.status == ExecutionStatus::Cancelled {
            return Ok(Some(workflow));
        }
        if matches!(
            workflow.status,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Timeout
                | ExecutionStatus::Abandoned
        ) {
            if parent_update.is_some() {
                return Err(crate::Error::InvalidState(format!(
                    "workflow execution {} completed before cancellation acquired its lock",
                    workflow.id
                )));
            }
            return Ok(Some(workflow));
        }
        if let Some((status, result)) = parent_update {
            let updated = sqlx::query(
                "UPDATE execution SET status = $2, result = COALESCE($3, result), updated = NOW() \
                 WHERE id = $1 \
                   AND status IN ('requested', 'scheduling', 'scheduled', 'running', 'canceling', 'cancelled')",
            )
            .bind(workflow.execution)
            .bind(status)
            .bind(result)
            .execute(&mut *conn)
            .await?;
            if updated.rows_affected() == 0 {
                return Err(crate::Error::InvalidState(format!(
                    "execution {} completed before workflow cancellation acquired its lock",
                    workflow.execution
                )));
            }
        }

        super::inquiry::InquiryRepository::cancel_pending_for_workflow(&mut *conn, id).await?;
        super::workflow_task_wait::WorkflowTaskWaitRepository::cancel_waiting_for_workflow(
            &mut *conn,
            id,
            serde_json::json!({"reason": "workflow cancelled"}),
        )
        .await?;
        let updated = Self::update(
            &mut *conn,
            id,
            UpdateWorkflowExecutionInput {
                status: Some(ExecutionStatus::Cancelled),
                error_message: Some(error_message.to_string()),
                current_tasks: Some(vec![]),
                ..Default::default()
            },
        )
        .await?;
        Ok(Some(updated))
    }

    pub async fn find_by_id_for_update<'e, E>(
        executor: E,
        id: Id,
    ) -> Result<Option<WorkflowExecution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowExecution>(
            "SELECT id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                    variables, task_graph, status, error_message, paused, pause_reason, created, updated
             FROM workflow_execution
             WHERE id = $1
             FOR UPDATE"
        )
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn create_or_get_by_execution<'e, E>(
        executor: E,
        input: CreateWorkflowExecutionInput,
    ) -> Result<WorkflowExecutionCreateOrGetResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let inserted = sqlx::query_as::<_, WorkflowExecution>(
            "INSERT INTO workflow_execution
             (execution, workflow_def, task_graph, variables, status)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (execution) DO NOTHING
             RETURNING id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                       variables, task_graph, status, error_message, paused, pause_reason, created, updated"
        )
        .bind(input.execution)
        .bind(input.workflow_def)
        .bind(&input.task_graph)
        .bind(&input.variables)
        .bind(input.status)
        .fetch_optional(executor)
        .await?;

        if let Some(workflow_execution) = inserted {
            return Ok(WorkflowExecutionCreateOrGetResult {
                workflow_execution,
                created: true,
            });
        }

        let workflow_execution = Self::find_by_execution(executor, input.execution)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "workflow_execution for parent execution {} disappeared after conflict",
                    input.execution
                )
            })?;

        Ok(WorkflowExecutionCreateOrGetResult {
            workflow_execution,
            created: false,
        })
    }

    /// Find workflow execution by the parent execution ID
    pub async fn find_by_execution<'e, E>(
        executor: E,
        execution_id: Id,
    ) -> Result<Option<WorkflowExecution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowExecution>(
            "SELECT id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                    variables, task_graph, status, error_message, paused, pause_reason, created, updated
             FROM workflow_execution
             WHERE execution = $1"
        )
        .bind(execution_id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    /// Find all workflow executions by status
    pub async fn find_by_status<'e, E>(
        executor: E,
        status: ExecutionStatus,
    ) -> Result<Vec<WorkflowExecution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowExecution>(
            "SELECT id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                    variables, task_graph, status, error_message, paused, pause_reason, created, updated
             FROM workflow_execution
             WHERE status = $1
             ORDER BY created DESC"
        )
        .bind(status)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    /// Find all paused workflow executions
    pub async fn find_paused<'e, E>(executor: E) -> Result<Vec<WorkflowExecution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowExecution>(
            "SELECT id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                    variables, task_graph, status, error_message, paused, pause_reason, created, updated
             FROM workflow_execution
             WHERE paused = true
             ORDER BY created DESC"
        )
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    /// Find workflow executions by workflow definition
    pub async fn find_by_workflow_def<'e, E>(
        executor: E,
        workflow_def_id: Id,
    ) -> Result<Vec<WorkflowExecution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, WorkflowExecution>(
            "SELECT id, execution, workflow_def, current_tasks, completed_tasks, failed_tasks, skipped_tasks,
                    variables, task_graph, status, error_message, paused, pause_reason, created, updated
             FROM workflow_execution
             WHERE workflow_def = $1
             ORDER BY created DESC"
        )
        .bind(workflow_def_id)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }
}
