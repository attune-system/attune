//! Execution repository for database operations

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::models::{enums::ExecutionStatus, execution::*, Id, JsonDict};
use crate::scheduling::{parse_worker_affinity, parse_worker_selector, parse_worker_tolerations};
use crate::trace_tag::default_execution_trace_tag;
use crate::Result;
use sqlx::{Executor, PgConnection, PgPool, Postgres, QueryBuilder, Row};
use tokio::time::{sleep, Duration};

use super::{ref_filter_like_pattern, Create, Delete, FindById, List, Repository, Update};

fn needs_enforcement_join(filters: &ExecutionSearchFilters) -> bool {
    filters.rule_ref.is_some() || filters.trigger_ref.is_some()
}

/// Filters for the [`ExecutionRepository::search`] query-builder method.
///
/// Every field is optional. When set, the corresponding `WHERE` clause is
/// appended to the query. Pagination (`limit`/`offset`) is always applied.
///
/// Filters that involve the `enforcement` table (`rule_ref`, `trigger_ref`)
/// cause a `LEFT JOIN enforcement` to be added automatically.
#[derive(Debug, Clone)]
pub struct ExecutionSearchFilters {
    pub status: Option<ExecutionStatus>,
    pub action_ref: Option<String>,
    pub pack_name: Option<String>,
    pub rule_ref: Option<String>,
    pub trigger_ref: Option<String>,
    pub trace_tag: Option<String>,
    pub executor: Option<Id>,
    pub result_contains: Option<String>,
    pub enforcement: Option<Id>,
    pub parent: Option<Id>,
    pub top_level_only: bool,
    pub include_total: bool,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ExecutionSearchFilters {
    fn default() -> Self {
        Self {
            status: None,
            action_ref: None,
            pack_name: None,
            rule_ref: None,
            trigger_ref: None,
            trace_tag: None,
            executor: None,
            result_contains: None,
            enforcement: None,
            parent: None,
            top_level_only: false,
            include_total: true,
            limit: 0,
            offset: 0,
        }
    }
}

/// Result of [`ExecutionRepository::search`].
///
/// Includes the matching rows and pagination metadata derived from the query
/// strategy. Exact totals are only populated when explicitly requested.
#[derive(Debug)]
pub struct ExecutionSearchResult {
    pub rows: Vec<ExecutionWithRefs>,
    pub total: Option<u64>,
    pub has_next: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkerExecutionLoad {
    pub worker_id: Id,
    pub requested: i64,
    pub scheduling: i64,
    pub scheduled: i64,
    pub running: i64,
    pub canceling: i64,
    pub total_active: i64,
}

#[derive(Debug, Clone)]
pub struct WorkflowTaskExecutionCreateOrGetResult {
    pub execution: Execution,
    pub created: bool,
}

#[derive(Debug, Clone)]
pub struct EnforcementExecutionCreateOrGetResult {
    pub execution: Execution,
    pub created: bool,
}

/// An execution row with optional `rule_ref` / `trigger_ref` populated from
/// the joined `enforcement` table. This avoids a separate in-memory lookup.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExecutionWithRefs {
    // — execution columns (same order as SELECT_COLUMNS) —
    pub id: Id,
    pub action: Option<Id>,
    pub action_ref: String,
    pub config: Option<JsonDict>,
    pub env_vars: Option<JsonDict>,
    pub parent: Option<Id>,
    pub enforcement: Option<Id>,
    pub executor: Option<Id>,
    pub permission_set_refs: Vec<String>,
    pub artifact_retention_policy: Option<crate::models::enums::RetentionPolicyType>,
    pub artifact_retention_limit: Option<i32>,
    pub worker_selector: Option<JsonDict>,
    pub worker_tolerations: Option<JsonDict>,
    pub worker_affinity: Option<JsonDict>,
    pub worker: Option<Id>,
    pub status: ExecutionStatus,
    pub trace_tag: Option<String>,
    pub result: Option<JsonDict>,
    pub retry_count: i32,
    pub max_retries: Option<i32>,
    pub retry_reason: Option<String>,
    pub original_execution: Option<Id>,
    pub started_at: Option<DateTime<Utc>>,
    pub timeout_seconds: Option<i32>,
    #[sqlx(json(nullable), default)]
    pub workflow_task: Option<WorkflowTaskMetadata>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    // — joined from enforcement —
    pub rule_ref: Option<String>,
    pub trigger_ref: Option<String>,
}

/// Column list for SELECT queries on the execution table.
///
/// Defined once to avoid drift between queries and the `Execution` model.
/// The execution table has a DB-only `workflow_def` column that is NOT in the
/// Rust struct, so `SELECT *` must never be used.
pub const SELECT_COLUMNS: &str = "\
    id, action, action_ref, config, env_vars, parent, enforcement, \
    executor, permission_set_refs, artifact_retention_policy, artifact_retention_limit, \
    worker_selector, worker_tolerations, worker_affinity, \
    worker, status, trace_tag, result, retry_count, max_retries, retry_reason, \
    original_execution, started_at, timeout_seconds, workflow_task, created, updated";

pub struct ExecutionRepository;

impl Repository for ExecutionRepository {
    type Entity = Execution;
    fn table_name() -> &'static str {
        "executions"
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreateExecutionInput {
    pub action: Option<Id>,
    pub action_ref: String,
    pub config: Option<JsonDict>,
    pub env_vars: Option<JsonDict>,
    pub parent: Option<Id>,
    pub enforcement: Option<Id>,
    pub executor: Option<Id>,
    pub permission_set_refs: Vec<String>,
    pub artifact_retention_policy: Option<crate::models::enums::RetentionPolicyType>,
    pub artifact_retention_limit: Option<i32>,
    pub worker_selector: Option<JsonDict>,
    pub worker_tolerations: Option<JsonDict>,
    pub worker_affinity: Option<JsonDict>,
    pub worker: Option<Id>,
    pub status: ExecutionStatus,
    pub result: Option<JsonDict>,
    /// Resolved execution timeout in seconds, snapshotted onto the execution row.
    pub timeout_seconds: Option<i32>,
    /// Immutable trace tag snapshotted onto the execution row at creation time.
    pub trace_tag: Option<String>,
    pub workflow_task: Option<WorkflowTaskMetadata>,
}

fn validate_execution_placement(input: &CreateExecutionInput) -> Result<()> {
    if let Some(worker_selector) = &input.worker_selector {
        parse_worker_selector(worker_selector)?;
    }
    if let Some(worker_tolerations) = &input.worker_tolerations {
        parse_worker_tolerations(worker_tolerations)?;
    }
    if let Some(worker_affinity) = &input.worker_affinity {
        parse_worker_affinity(worker_affinity)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct UpdateExecutionInput {
    pub status: Option<ExecutionStatus>,
    pub result: Option<JsonDict>,
    pub executor: Option<Id>,
    pub worker: Option<Id>,
    pub started_at: Option<DateTime<Utc>>,
    pub workflow_task: Option<WorkflowTaskMetadata>,
}

impl From<Execution> for UpdateExecutionInput {
    fn from(execution: Execution) -> Self {
        Self {
            status: Some(execution.status),
            result: execution.result,
            executor: execution.executor,
            worker: execution.worker,
            started_at: execution.started_at,
            workflow_task: execution.workflow_task,
        }
    }
}

#[async_trait::async_trait]
impl FindById for ExecutionRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM execution WHERE id = $1");
        sqlx::query_as::<_, Execution>(&sql)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for ExecutionRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql =
            format!("SELECT {SELECT_COLUMNS} FROM execution ORDER BY created DESC LIMIT 1000");
        sqlx::query_as::<_, Execution>(&sql)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for ExecutionRepository {
    type CreateInput = CreateExecutionInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut input = input;
        if input.trace_tag.is_none() {
            input.trace_tag = Some(default_execution_trace_tag(
                &input.action_ref,
                Utc::now().timestamp_millis(),
            )?);
        }
        validate_execution_placement(&input)?;
        let sql = format!(
            "INSERT INTO execution \
             (action, action_ref, config, env_vars, parent, enforcement, executor, permission_set_refs, \
              artifact_retention_policy, artifact_retention_limit, worker_selector, worker_tolerations, \
              worker_affinity, worker, status, trace_tag, result, timeout_seconds, workflow_task) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19) \
             RETURNING {SELECT_COLUMNS}"
        );
        sqlx::query_as::<_, Execution>(&sql)
            .bind(input.action)
            .bind(&input.action_ref)
            .bind(&input.config)
            .bind(&input.env_vars)
            .bind(input.parent)
            .bind(input.enforcement)
            .bind(input.executor)
            .bind(&input.permission_set_refs)
            .bind(input.artifact_retention_policy)
            .bind(input.artifact_retention_limit)
            .bind(&input.worker_selector)
            .bind(&input.worker_tolerations)
            .bind(&input.worker_affinity)
            .bind(input.worker)
            .bind(input.status)
            .bind(&input.trace_tag)
            .bind(&input.result)
            .bind(input.timeout_seconds)
            .bind(sqlx::types::Json(&input.workflow_task))
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for ExecutionRepository {
    type UpdateInput = UpdateExecutionInput;
    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.status.is_none()
            && input.result.is_none()
            && input.executor.is_none()
            && input.worker.is_none()
            && input.started_at.is_none()
            && input.workflow_task.is_none()
        {
            return Self::get_by_id(executor, id).await;
        }

        Self::update_with_locator(executor, input, |query| {
            query.push(" WHERE id = ").push_bind(id);
        })
        .await
    }
}

impl ExecutionRepository {
    pub async fn create_retry<'e, E>(
        executor: E,
        input: CreateExecutionInput,
        retry_count: i32,
        max_retries: Option<i32>,
        retry_reason: Option<String>,
        original_execution: Id,
    ) -> Result<Execution>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut input = input;
        if input.trace_tag.is_none() {
            input.trace_tag = Some(default_execution_trace_tag(
                &input.action_ref,
                Utc::now().timestamp_millis(),
            )?);
        }
        validate_execution_placement(&input)?;
        let sql = format!(
            "INSERT INTO execution \
             (action, action_ref, config, env_vars, parent, enforcement, executor, permission_set_refs, \
              artifact_retention_policy, artifact_retention_limit, worker_selector, worker_tolerations, \
              worker_affinity, worker, status, trace_tag, result, timeout_seconds, workflow_task, \
              retry_count, max_retries, retry_reason, original_execution) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23) \
             RETURNING {SELECT_COLUMNS}"
        );
        sqlx::query_as::<_, Execution>(&sql)
            .bind(input.action)
            .bind(&input.action_ref)
            .bind(&input.config)
            .bind(&input.env_vars)
            .bind(input.parent)
            .bind(input.enforcement)
            .bind(input.executor)
            .bind(&input.permission_set_refs)
            .bind(input.artifact_retention_policy)
            .bind(input.artifact_retention_limit)
            .bind(&input.worker_selector)
            .bind(&input.worker_tolerations)
            .bind(&input.worker_affinity)
            .bind(input.worker)
            .bind(input.status)
            .bind(&input.trace_tag)
            .bind(&input.result)
            .bind(input.timeout_seconds)
            .bind(sqlx::types::Json(&input.workflow_task))
            .bind(retry_count)
            .bind(max_retries)
            .bind(&retry_reason)
            .bind(original_execution)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_top_level_by_enforcement<'e, E>(
        executor: E,
        enforcement_id: Id,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} \
             FROM execution \
             WHERE enforcement = $1
               AND parent IS NULL
               AND (config IS NULL OR NOT (config ? 'retry_of')) \
             ORDER BY created ASC \
             LIMIT 1"
        );

        sqlx::query_as::<_, Execution>(&sql)
            .bind(enforcement_id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn create_top_level_for_enforcement_if_absent<'e, E>(
        executor: E,
        input: CreateExecutionInput,
        enforcement_id: Id,
    ) -> Result<EnforcementExecutionCreateOrGetResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let mut input = input;
        if input.trace_tag.is_none() {
            input.trace_tag = Some(default_execution_trace_tag(
                &input.action_ref,
                Utc::now().timestamp_millis(),
            )?);
        }
        validate_execution_placement(&input)?;
        let inserted = sqlx::query_as::<_, Execution>(&format!(
            "INSERT INTO execution \
             (action, action_ref, config, env_vars, parent, enforcement, executor, permission_set_refs, \
              worker_selector, worker_tolerations, worker_affinity, worker, status, trace_tag, result, timeout_seconds, workflow_task) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) \
             ON CONFLICT (enforcement)
             WHERE enforcement IS NOT NULL
               AND parent IS NULL
               AND (config IS NULL OR NOT (config ? 'retry_of'))
             DO NOTHING \
             RETURNING {SELECT_COLUMNS}"
        ))
        .bind(input.action)
        .bind(&input.action_ref)
        .bind(&input.config)
        .bind(&input.env_vars)
        .bind(input.parent)
        .bind(input.enforcement)
        .bind(input.executor)
        .bind(&input.permission_set_refs)
        .bind(&input.worker_selector)
        .bind(&input.worker_tolerations)
        .bind(&input.worker_affinity)
        .bind(input.worker)
        .bind(input.status)
        .bind(&input.trace_tag)
        .bind(&input.result)
        .bind(input.timeout_seconds)
        .bind(sqlx::types::Json(&input.workflow_task))
        .fetch_optional(executor)
        .await?;

        if let Some(execution) = inserted {
            return Ok(EnforcementExecutionCreateOrGetResult {
                execution,
                created: true,
            });
        }

        let execution = Self::find_top_level_by_enforcement(executor, enforcement_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "top-level execution for enforcement {} disappeared after dedupe conflict",
                    enforcement_id
                )
            })?;

        Ok(EnforcementExecutionCreateOrGetResult {
            execution,
            created: false,
        })
    }

    async fn claim_workflow_task_dispatch<'e, E>(
        executor: E,
        workflow_execution_id: Id,
        task_name: &str,
        task_index: Option<i32>,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let inserted: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO workflow_task_dispatch (workflow_execution, task_name, task_index)
             VALUES ($1, $2, $3)
             ON CONFLICT (workflow_execution, task_name, COALESCE(task_index, -1)) DO NOTHING
             RETURNING id",
        )
        .bind(workflow_execution_id)
        .bind(task_name)
        .bind(task_index)
        .fetch_optional(executor)
        .await?;

        Ok(inserted.is_some())
    }

    async fn assign_workflow_task_dispatch_execution<'e, E>(
        executor: E,
        workflow_execution_id: Id,
        task_name: &str,
        task_index: Option<i32>,
        execution_id: Id,
    ) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query(
            "UPDATE workflow_task_dispatch
             SET execution_id = COALESCE(execution_id, $4)
             WHERE workflow_execution = $1
               AND task_name = $2
               AND task_index IS NOT DISTINCT FROM $3",
        )
        .bind(workflow_execution_id)
        .bind(task_name)
        .bind(task_index)
        .bind(execution_id)
        .execute(executor)
        .await?;

        Ok(())
    }

    async fn lock_workflow_task_dispatch<'e, E>(
        executor: E,
        workflow_execution_id: Id,
        task_name: &str,
        task_index: Option<i32>,
    ) -> Result<Option<Option<Id>>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let row: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT execution_id
             FROM workflow_task_dispatch
             WHERE workflow_execution = $1
               AND task_name = $2
               AND task_index IS NOT DISTINCT FROM $3
             FOR UPDATE",
        )
        .bind(workflow_execution_id)
        .bind(task_name)
        .bind(task_index)
        .fetch_optional(executor)
        .await?;

        // Map the outer Option to distinguish three cases:
        // - None            → no row exists
        // - Some(None)      → row exists but execution_id is still NULL (mid-creation)
        // - Some(Some(id))  → row exists with a completed execution_id
        Ok(row.map(|(execution_id,)| execution_id))
    }

    async fn create_workflow_task_if_absent_in_conn(
        conn: &mut PgConnection,
        input: CreateExecutionInput,
        workflow_execution_id: Id,
        task_name: &str,
        task_index: Option<i32>,
    ) -> Result<WorkflowTaskExecutionCreateOrGetResult> {
        let claimed = Self::claim_workflow_task_dispatch(
            &mut *conn,
            workflow_execution_id,
            task_name,
            task_index,
        )
        .await?;

        if claimed {
            let execution = Self::create(&mut *conn, input).await?;
            Self::assign_workflow_task_dispatch_execution(
                &mut *conn,
                workflow_execution_id,
                task_name,
                task_index,
                execution.id,
            )
            .await?;

            return Ok(WorkflowTaskExecutionCreateOrGetResult {
                execution,
                created: true,
            });
        }

        let dispatch_state = Self::lock_workflow_task_dispatch(
            &mut *conn,
            workflow_execution_id,
            task_name,
            task_index,
        )
        .await?;

        match dispatch_state {
            Some(Some(existing_execution_id)) => {
                // Row exists with execution_id — return the existing execution.
                let execution = Self::find_by_id(&mut *conn, existing_execution_id)
                    .await?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "workflow child execution {} missing for workflow_execution {} task '{}' index {:?}",
                            existing_execution_id,
                            workflow_execution_id,
                            task_name,
                            task_index
                        )
                    })?;

                Ok(WorkflowTaskExecutionCreateOrGetResult {
                    execution,
                    created: false,
                })
            }

            Some(None) => {
                // Row exists but execution_id is still NULL: another transaction is
                // mid-creation (between claim and assign). Retry until it's filled in.
                // If the original creator's transaction rolled back, the row also
                // disappears — handled by the `None` branch inside the loop.
                'wait: {
                    for _ in 0..20_u32 {
                        sleep(Duration::from_millis(50)).await;
                        match Self::lock_workflow_task_dispatch(
                            &mut *conn,
                            workflow_execution_id,
                            task_name,
                            task_index,
                        )
                        .await?
                        {
                            Some(Some(execution_id)) => {
                                let execution =
                                    Self::find_by_id(&mut *conn, execution_id).await?.ok_or_else(
                                        || {
                                            anyhow::anyhow!(
                                                "workflow child execution {} missing for workflow_execution {} task '{}' index {:?}",
                                                execution_id,
                                                workflow_execution_id,
                                                task_name,
                                                task_index
                                            )
                                        },
                                    )?;
                                return Ok(WorkflowTaskExecutionCreateOrGetResult {
                                    execution,
                                    created: false,
                                });
                            }
                            Some(None) => {}     // still NULL, keep waiting
                            None => break 'wait, // row rolled back; fall through to re-claim
                        }
                    }
                    // Exhausted all retries without the execution_id being set.
                    return Err(anyhow::anyhow!(
                        "Timed out waiting for workflow task dispatch execution_id to be set \
                         for workflow_execution {} task '{}' index {:?}",
                        workflow_execution_id,
                        task_name,
                        task_index
                    )
                    .into());
                }

                // Row disappeared (original creator rolled back) — re-claim and create.
                let re_claimed = Self::claim_workflow_task_dispatch(
                    &mut *conn,
                    workflow_execution_id,
                    task_name,
                    task_index,
                )
                .await?;
                if !re_claimed {
                    return Err(anyhow::anyhow!(
                        "Workflow task dispatch for workflow_execution {} task '{}' index {:?} \
                         was reclaimed by another executor after rollback",
                        workflow_execution_id,
                        task_name,
                        task_index
                    )
                    .into());
                }
                let execution = Self::create(&mut *conn, input).await?;
                Self::assign_workflow_task_dispatch_execution(
                    &mut *conn,
                    workflow_execution_id,
                    task_name,
                    task_index,
                    execution.id,
                )
                .await?;
                Ok(WorkflowTaskExecutionCreateOrGetResult {
                    execution,
                    created: true,
                })
            }

            None => {
                // No row at all — the original INSERT was rolled back before we arrived.
                // Attempt to re-claim and create as if this were a fresh dispatch.
                let re_claimed = Self::claim_workflow_task_dispatch(
                    &mut *conn,
                    workflow_execution_id,
                    task_name,
                    task_index,
                )
                .await?;
                if !re_claimed {
                    return Err(anyhow::anyhow!(
                        "Workflow task dispatch for workflow_execution {} task '{}' index {:?} \
                         was claimed by another executor",
                        workflow_execution_id,
                        task_name,
                        task_index
                    )
                    .into());
                }
                let execution = Self::create(&mut *conn, input).await?;
                Self::assign_workflow_task_dispatch_execution(
                    &mut *conn,
                    workflow_execution_id,
                    task_name,
                    task_index,
                    execution.id,
                )
                .await?;
                Ok(WorkflowTaskExecutionCreateOrGetResult {
                    execution,
                    created: true,
                })
            }
        }
    }

    pub async fn create_workflow_task_if_absent(
        pool: &PgPool,
        input: CreateExecutionInput,
        workflow_execution_id: Id,
        task_name: &str,
        task_index: Option<i32>,
    ) -> Result<WorkflowTaskExecutionCreateOrGetResult> {
        let mut conn = pool.acquire().await?;
        sqlx::query("BEGIN").execute(&mut *conn).await?;

        let result = Self::create_workflow_task_if_absent_in_conn(
            &mut conn,
            input,
            workflow_execution_id,
            task_name,
            task_index,
        )
        .await;

        match result {
            Ok(result) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(result)
            }
            Err(err) => {
                sqlx::query("ROLLBACK").execute(&mut *conn).await?;
                Err(err)
            }
        }
    }

    pub async fn create_workflow_task_if_absent_with_conn(
        conn: &mut PgConnection,
        input: CreateExecutionInput,
        workflow_execution_id: Id,
        task_name: &str,
        task_index: Option<i32>,
    ) -> Result<WorkflowTaskExecutionCreateOrGetResult> {
        Self::create_workflow_task_if_absent_in_conn(
            conn,
            input,
            workflow_execution_id,
            task_name,
            task_index,
        )
        .await
    }

    pub async fn claim_for_scheduling<'e, E>(
        executor: E,
        id: Id,
        claiming_executor: Option<Id>,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "UPDATE execution \
             SET status = $2, executor = COALESCE($3, executor), updated = NOW() \
             WHERE id = $1 AND status = $4 \
             RETURNING {SELECT_COLUMNS}"
        );

        sqlx::query_as::<_, Execution>(&sql)
            .bind(id)
            .bind(ExecutionStatus::Scheduling)
            .bind(claiming_executor)
            .bind(ExecutionStatus::Requested)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn reclaim_stale_scheduling<'e, E>(
        executor: E,
        id: Id,
        claiming_executor: Option<Id>,
        stale_before: DateTime<Utc>,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "UPDATE execution \
             SET executor = COALESCE($2, executor), updated = NOW() \
             WHERE id = $1 AND status = $3 AND updated <= $4 \
             RETURNING {SELECT_COLUMNS}"
        );

        sqlx::query_as::<_, Execution>(&sql)
            .bind(id)
            .bind(claiming_executor)
            .bind(ExecutionStatus::Scheduling)
            .bind(stale_before)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn update_if_status<'e, E>(
        executor: E,
        id: Id,
        expected_status: ExecutionStatus,
        input: UpdateExecutionInput,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.status.is_none()
            && input.result.is_none()
            && input.executor.is_none()
            && input.worker.is_none()
            && input.started_at.is_none()
            && input.workflow_task.is_none()
        {
            return Self::find_by_id(executor, id).await;
        }

        Self::update_with_locator_optional(executor, input, |query| {
            query.push(" WHERE id = ").push_bind(id);
            query.push(" AND status = ").push_bind(expected_status);
        })
        .await
    }

    pub async fn update_if_status_and_updated_before<'e, E>(
        executor: E,
        id: Id,
        expected_status: ExecutionStatus,
        stale_before: DateTime<Utc>,
        input: UpdateExecutionInput,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.status.is_none()
            && input.result.is_none()
            && input.executor.is_none()
            && input.worker.is_none()
            && input.started_at.is_none()
            && input.workflow_task.is_none()
        {
            return Self::find_by_id(executor, id).await;
        }

        Self::update_with_locator_optional(executor, input, |query| {
            query.push(" WHERE id = ").push_bind(id);
            query.push(" AND status = ").push_bind(expected_status);
            query.push(" AND updated < ").push_bind(stale_before);
        })
        .await
    }

    pub async fn update_if_status_and_updated_at<'e, E>(
        executor: E,
        id: Id,
        expected_status: ExecutionStatus,
        expected_updated: DateTime<Utc>,
        input: UpdateExecutionInput,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.status.is_none()
            && input.result.is_none()
            && input.executor.is_none()
            && input.worker.is_none()
            && input.started_at.is_none()
            && input.workflow_task.is_none()
        {
            return Self::find_by_id(executor, id).await;
        }

        Self::update_with_locator_optional(executor, input, |query| {
            query.push(" WHERE id = ").push_bind(id);
            query.push(" AND status = ").push_bind(expected_status);
            query.push(" AND updated = ").push_bind(expected_updated);
        })
        .await
    }

    pub async fn revert_scheduled_to_requested<'e, E>(
        executor: E,
        id: Id,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "UPDATE execution \
             SET status = $2, worker = NULL, executor = NULL, updated = NOW() \
             WHERE id = $1 AND status = $3 \
             RETURNING {SELECT_COLUMNS}"
        );

        sqlx::query_as::<_, Execution>(&sql)
            .bind(id)
            .bind(ExecutionStatus::Requested)
            .bind(ExecutionStatus::Scheduled)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    async fn update_with_locator<'e, E, F>(
        executor: E,
        input: UpdateExecutionInput,
        where_clause: F,
    ) -> Result<Execution>
    where
        E: Executor<'e, Database = Postgres> + 'e,
        F: FnOnce(&mut QueryBuilder<'_, Postgres>),
    {
        let mut query = QueryBuilder::new("UPDATE execution SET ");
        let mut has_updates = false;

        if let Some(status) = input.status {
            query.push("status = ").push_bind(status);
            has_updates = true;
        }
        if let Some(result) = &input.result {
            if has_updates {
                query.push(", ");
            }
            query.push("result = ").push_bind(result);
            has_updates = true;
        }
        if let Some(executor_id) = input.executor {
            if has_updates {
                query.push(", ");
            }
            query.push("executor = ").push_bind(executor_id);
            has_updates = true;
        }
        if let Some(worker_id) = input.worker {
            if has_updates {
                query.push(", ");
            }
            query.push("worker = ").push_bind(worker_id);
            has_updates = true;
        }
        if let Some(started_at) = input.started_at {
            if has_updates {
                query.push(", ");
            }
            query.push("started_at = ").push_bind(started_at);
            has_updates = true;
        }
        if let Some(workflow_task) = &input.workflow_task {
            if has_updates {
                query.push(", ");
            }
            query
                .push("workflow_task = ")
                .push_bind(sqlx::types::Json(workflow_task));
        }

        query.push(", updated = NOW()");
        where_clause(&mut query);
        query.push(" RETURNING ");
        query.push(SELECT_COLUMNS);

        query
            .build_query_as::<Execution>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    async fn update_with_locator_optional<'e, E, F>(
        executor: E,
        input: UpdateExecutionInput,
        where_clause: F,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
        F: FnOnce(&mut QueryBuilder<'_, Postgres>),
    {
        let mut query = QueryBuilder::new("UPDATE execution SET ");
        let mut has_updates = false;

        if let Some(status) = input.status {
            query.push("status = ").push_bind(status);
            has_updates = true;
        }
        if let Some(result) = &input.result {
            if has_updates {
                query.push(", ");
            }
            query.push("result = ").push_bind(result);
            has_updates = true;
        }
        if let Some(executor_id) = input.executor {
            if has_updates {
                query.push(", ");
            }
            query.push("executor = ").push_bind(executor_id);
            has_updates = true;
        }
        if let Some(worker_id) = input.worker {
            if has_updates {
                query.push(", ");
            }
            query.push("worker = ").push_bind(worker_id);
            has_updates = true;
        }
        if let Some(started_at) = input.started_at {
            if has_updates {
                query.push(", ");
            }
            query.push("started_at = ").push_bind(started_at);
            has_updates = true;
        }
        if let Some(workflow_task) = &input.workflow_task {
            if has_updates {
                query.push(", ");
            }
            query
                .push("workflow_task = ")
                .push_bind(sqlx::types::Json(workflow_task));
        }

        query.push(", updated = NOW()");
        where_clause(&mut query);
        query.push(" RETURNING ");
        query.push(SELECT_COLUMNS);

        query
            .build_query_as::<Execution>()
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Update an execution using the loaded row's primary key.
    pub async fn update_loaded<'e, E>(
        executor: E,
        execution: &Execution,
        input: UpdateExecutionInput,
    ) -> Result<Execution>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.status.is_none()
            && input.result.is_none()
            && input.executor.is_none()
            && input.worker.is_none()
            && input.started_at.is_none()
            && input.workflow_task.is_none()
        {
            return Ok(execution.clone());
        }

        Self::update_with_locator(executor, input, |query| {
            query.push(" WHERE id = ").push_bind(execution.id);
        })
        .await
    }
}

#[async_trait::async_trait]
impl Delete for ExecutionRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            r#"
            WITH deleted_execution AS (
                DELETE FROM execution
                WHERE id = $1
                RETURNING id
            ),
            deleted_config_secrets AS (
                DELETE FROM execution_secret_value
                WHERE entity_type = 'execution_config'
                  AND entity_id IN (SELECT id FROM deleted_execution)
            ),
            deleted_result_secrets AS (
                DELETE FROM execution_secret_value
                WHERE entity_type = 'execution_result'
                  AND entity_id IN (SELECT id FROM deleted_execution)
            )
            SELECT COUNT(*) AS deleted_count FROM deleted_execution
            "#,
        )
        .bind(id)
        .fetch_one(executor)
        .await?;
        let deleted_count: i64 = result.get("deleted_count");
        Ok(deleted_count > 0)
    }
}

impl ExecutionRepository {
    /// Return a current execution load snapshot for the given worker IDs.
    pub async fn current_load_by_worker_ids(
        pool: &PgPool,
        worker_ids: &[Id],
    ) -> Result<Vec<WorkerExecutionLoad>> {
        if worker_ids.is_empty() {
            return Ok(Vec::new());
        }

        sqlx::query_as::<_, WorkerExecutionLoad>(
            r#"
            SELECT
                worker AS worker_id,
                COUNT(*) FILTER (WHERE status = 'requested')::BIGINT AS requested,
                COUNT(*) FILTER (WHERE status = 'scheduling')::BIGINT AS scheduling,
                COUNT(*) FILTER (WHERE status = 'scheduled')::BIGINT AS scheduled,
                COUNT(*) FILTER (WHERE status = 'running')::BIGINT AS running,
                COUNT(*) FILTER (WHERE status = 'canceling')::BIGINT AS canceling,
                COUNT(*) FILTER (
                    WHERE status IN ('requested', 'scheduling', 'scheduled', 'running', 'canceling')
                )::BIGINT AS total_active
            FROM execution
            WHERE worker = ANY($1)
              AND status IN ('requested', 'scheduling', 'scheduled', 'running', 'canceling')
            GROUP BY worker
            "#,
        )
        .bind(worker_ids)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_status<'e, E>(
        executor: E,
        status: ExecutionStatus,
    ) -> Result<Vec<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM execution WHERE status = $1 ORDER BY created DESC"
        );
        sqlx::query_as::<_, Execution>(&sql)
            .bind(status)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_enforcement<'e, E>(
        executor: E,
        enforcement_id: Id,
    ) -> Result<Vec<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM execution WHERE enforcement = $1 ORDER BY created DESC"
        );
        sqlx::query_as::<_, Execution>(&sql)
            .bind(enforcement_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_workflow_task<'e, E>(
        executor: E,
        workflow_execution_id: Id,
        task_name: &str,
        task_index: Option<i32>,
    ) -> Result<Option<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} \
             FROM execution \
             WHERE workflow_task->>'workflow_execution' = $1::text \
               AND workflow_task->>'task_name' = $2 \
               AND (workflow_task->>'task_index')::int IS NOT DISTINCT FROM $3 \
             ORDER BY created ASC \
             LIMIT 1"
        );

        sqlx::query_as::<_, Execution>(&sql)
            .bind(workflow_execution_id.to_string())
            .bind(task_name)
            .bind(task_index)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Find all child executions for a given parent execution ID.
    ///
    /// Returns child executions ordered by creation time (ascending),
    /// which is the natural task execution order for workflows.
    pub async fn find_by_parent<'e, E>(executor: E, parent_id: Id) -> Result<Vec<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM execution WHERE parent = $1 ORDER BY created ASC"
        );
        sqlx::query_as::<_, Execution>(&sql)
            .bind(parent_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Bulk-fetch executions by ID, in a single round trip.
    ///
    /// Used by callers that need to resolve several linked executions (e.g.
    /// for read-visibility redaction of a paginated list) without issuing
    /// one `find_by_id` query per row.
    pub async fn find_by_ids<'e, E>(executor: E, ids: &[Id]) -> Result<Vec<Execution>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!("SELECT {SELECT_COLUMNS} FROM execution WHERE id = ANY($1)");
        sqlx::query_as::<_, Execution>(&sql)
            .bind(ids)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Batch-resolve ancestor executor identity IDs for a set of execution IDs.
    ///
    /// For each ID in `execution_ids`, walks up the `parent` chain (excluding
    /// the execution itself) and collects the `executor` identity ID of
    /// every ancestor along the way. Returns a map from execution ID to the
    /// sorted, deduplicated list of ancestor executor IDs; execution IDs with
    /// no ancestors (or none with a non-null `executor`) are simply absent
    /// from the map.
    ///
    /// This resolves the whole set with a single recursive query instead of
    /// walking the parent chain one round trip per ancestor level per
    /// execution.
    pub async fn ancestor_executor_ids_by_ids<'e, E>(
        executor: E,
        execution_ids: &[Id],
    ) -> Result<HashMap<Id, Vec<Id>>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if execution_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = sqlx::query(
            "WITH RECURSIVE ancestor_chain AS ( \
                 SELECT e.id AS leaf_id, p.executor AS ancestor_executor, p.parent AS next_parent \
                 FROM execution e \
                 INNER JOIN execution p ON p.id = e.parent \
                 WHERE e.id = ANY($1) \
                 UNION ALL \
                 SELECT ac.leaf_id, p.executor, p.parent \
                 FROM ancestor_chain ac \
                 INNER JOIN execution p ON p.id = ac.next_parent \
             ) \
             SELECT leaf_id, ancestor_executor FROM ancestor_chain WHERE ancestor_executor IS NOT NULL",
        )
        .bind(execution_ids)
        .fetch_all(executor)
        .await?;

        let mut result: HashMap<Id, Vec<Id>> = HashMap::new();
        for row in rows {
            let leaf_id: Id = row.try_get("leaf_id")?;
            let ancestor_executor: Id = row.try_get("ancestor_executor")?;
            result.entry(leaf_id).or_default().push(ancestor_executor);
        }
        for ids in result.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        Ok(result)
    }

    /// Search executions with all filters pushed into SQL.
    ///
    /// Builds a dynamic query with only the WHERE clauses that apply,
    /// a LEFT JOIN on `enforcement` when `rule_ref` or `trigger_ref` filters
    /// are present (or always, to populate those columns on the result),
    /// and proper LIMIT/OFFSET so pagination is server-side.
    ///
    /// Returns the matching page, plus either exact totals or an inferred
    /// `has_next` flag depending on the query mode.
    pub async fn search<'e, E>(
        db: E,
        filters: &ExecutionSearchFilters,
    ) -> Result<ExecutionSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let prefixed_select = SELECT_COLUMNS
            .split(", ")
            .map(|col| format!("e.{col}"))
            .collect::<Vec<_>>()
            .join(", ");

        let select_clause =
            format!("{prefixed_select}, enf.rule_ref AS rule_ref, enf.trigger_ref AS trigger_ref");

        let data_from_clause =
            "FROM execution e LEFT JOIN enforcement enf ON e.enforcement = enf.id";
        let count_from_clause = if needs_enforcement_join(filters) {
            "FROM execution e LEFT JOIN enforcement enf ON e.enforcement = enf.id"
        } else {
            "FROM execution e"
        };

        // ── Build WHERE clauses ──────────────────────────────────────────
        let mut conditions: Vec<String> = Vec::new();

        // We'll collect bind values to push into the QueryBuilder afterwards.
        // Because QueryBuilder doesn't let us interleave raw SQL and binds in
        // arbitrary order easily, we build the SQL string with numbered $N
        // placeholders and then bind in order.

        // Track the next placeholder index ($1, $2, …).
        // We can't use QueryBuilder's push_bind because we need the COUNT(*)
        // query to share the same WHERE clause text. Instead we build the
        // clause once and execute both queries with manual binds.

        // ── Use QueryBuilder for the data query ──────────────────────────
        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {select_clause} {data_from_clause}"));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT COUNT(*) AS total {count_from_clause}"));

        // Helper: append the same condition to both builders.
        // We need a tiny state machine since push_bind moves the value.
        macro_rules! push_condition {
            ($cond_prefix:expr, $value:expr) => {{
                let needs_where = conditions.is_empty();
                conditions.push(String::new()); // just to track count
                if needs_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
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

        macro_rules! push_like_condition {
            ($cond_prefix:expr, $value:expr) => {{
                let needs_where = conditions.is_empty();
                conditions.push(String::new()); // just to track count
                if needs_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push($cond_prefix);
                qb.push_bind($value.clone());
                qb.push(r" ESCAPE '\'");
                count_qb.push($cond_prefix);
                count_qb.push_bind($value);
                count_qb.push(r" ESCAPE '\'");
            }};
        }

        macro_rules! push_raw_condition {
            ($cond:expr) => {{
                let needs_where = conditions.is_empty();
                conditions.push(String::new());
                if needs_where {
                    qb.push(concat!(" WHERE ", $cond));
                    count_qb.push(concat!(" WHERE ", $cond));
                } else {
                    qb.push(concat!(" AND ", $cond));
                    count_qb.push(concat!(" AND ", $cond));
                }
            }};
        }

        if let Some(status) = &filters.status {
            push_condition!("e.status = ", *status);
        }
        if let Some(action_ref) = &filters.action_ref {
            if let Some(pattern) = ref_filter_like_pattern(action_ref) {
                push_like_condition!("e.action_ref LIKE ", pattern);
            } else {
                push_condition!("e.action_ref = ", action_ref.clone());
            }
        }
        if let Some(pack_name) = &filters.pack_name {
            let pattern = ref_filter_like_pattern(&format!("{pack_name}.*"))
                .expect("pack wildcard pattern contains '*'");
            push_like_condition!("e.action_ref LIKE ", pattern);
        }
        if let Some(enforcement_id) = filters.enforcement {
            push_condition!("e.enforcement = ", enforcement_id);
        }
        if let Some(parent_id) = filters.parent {
            push_condition!("e.parent = ", parent_id);
        }
        if filters.top_level_only {
            push_raw_condition!("e.parent IS NULL");
        }
        if let Some(executor_id) = filters.executor {
            push_condition!("e.executor = ", executor_id);
        }
        if let Some(rule_ref) = &filters.rule_ref {
            if let Some(pattern) = ref_filter_like_pattern(rule_ref) {
                push_like_condition!("enf.rule_ref LIKE ", pattern);
            } else {
                push_condition!("enf.rule_ref = ", rule_ref.clone());
            }
        }
        if let Some(trigger_ref) = &filters.trigger_ref {
            if let Some(pattern) = ref_filter_like_pattern(trigger_ref) {
                push_like_condition!("enf.trigger_ref LIKE ", pattern);
            } else {
                push_condition!("enf.trigger_ref = ", trigger_ref.clone());
            }
        }
        if let Some(trace_tag) = &filters.trace_tag {
            push_condition!("e.trace_tag = ", trace_tag.clone());
        }
        if let Some(search) = &filters.result_contains {
            let pattern = format!("%{}%", search.to_lowercase());
            push_condition!("LOWER(e.result::text) LIKE ", pattern);
        }

        let total = if filters.include_total {
            let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;
            Some(total.max(0) as u64)
        } else {
            None
        };

        // ── Data query with ORDER BY + pagination ────────────────────────
        qb.push(" ORDER BY e.created DESC");
        qb.push(" LIMIT ");
        let query_limit = if filters.include_total {
            filters.limit
        } else {
            filters.limit.saturating_add(1)
        };
        qb.push_bind(query_limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let mut rows: Vec<ExecutionWithRefs> = qb.build_query_as().fetch_all(db).await?;
        let has_next = if let Some(total) = total {
            filters.offset as u64 + (rows.len() as u64) < total
        } else if rows.len() > filters.limit as usize {
            rows.truncate(filters.limit as usize);
            true
        } else {
            false
        };

        Ok(ExecutionSearchResult {
            rows,
            total,
            has_next,
        })
    }

    pub async fn list_by_trace_tag<'e, E>(db: E, trace_tag: &str) -> Result<Vec<ExecutionWithRefs>>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let prefixed_select = SELECT_COLUMNS
            .split(", ")
            .map(|col| format!("e.{col}"))
            .collect::<Vec<_>>()
            .join(", ");
        let select_clause =
            format!("{prefixed_select}, enf.rule_ref AS rule_ref, enf.trigger_ref AS trigger_ref");
        let query = format!(
            "SELECT {select_clause} \
             FROM execution e \
             LEFT JOIN enforcement enf ON e.enforcement = enf.id \
             WHERE e.trace_tag = $1 \
             ORDER BY e.created ASC, e.id ASC"
        );
        sqlx::query_as::<_, ExecutionWithRefs>(&query)
            .bind(trace_tag)
            .fetch_all(db)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::{needs_enforcement_join, ExecutionSearchFilters};

    #[test]
    fn enforcement_join_only_needed_for_rule_or_trigger_filters() {
        assert!(!needs_enforcement_join(&ExecutionSearchFilters::default()));
        assert!(needs_enforcement_join(&ExecutionSearchFilters {
            rule_ref: Some("core.rule".to_string()),
            ..Default::default()
        }));
        assert!(needs_enforcement_join(&ExecutionSearchFilters {
            trigger_ref: Some("core.trigger".to_string()),
            ..Default::default()
        }));
        assert!(!needs_enforcement_join(&ExecutionSearchFilters {
            enforcement: Some(42),
            top_level_only: true,
            ..Default::default()
        }));
    }
}
