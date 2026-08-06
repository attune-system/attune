//! Durable cache iteration state for workflow tasks.

use crate::{
    models::{
        ExecutionStatus, Id, WorkflowCacheIteration, WorkflowCacheIterationState,
        WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS,
    },
    Error, Result,
};
use chrono::{Duration, Utc};
use sqlx::{Executor, FromRow, PgPool, Postgres};

use super::{FindById, Repository};

pub struct WorkflowCacheIterationRepository;

impl Repository for WorkflowCacheIterationRepository {
    type Entity = WorkflowCacheIteration;

    fn table_name() -> &'static str {
        "workflow_cache_iteration"
    }
}

#[derive(Debug, Clone)]
pub struct CreateWorkflowCacheIterationInput {
    pub workflow_execution: Id,
    pub task_name: String,
    pub namespace: Id,
    pub generation: Id,
    pub page_size: i32,
    pub batch_size: i32,
    pub concurrency: i32,
}

#[derive(Debug, Clone)]
pub struct UpdateWorkflowCacheIterationProgressInput {
    pub last_external_id: String,
    pub next_batch_index: i64,
    pub scanned_count: i64,
    pub dispatched_count: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct StaleSyntheticCacheIterationCompletion {
    pub execution_id: Id,
    pub action_id: Option<Id>,
    pub action_ref: String,
    pub status: ExecutionStatus,
    pub result: Option<serde_json::Value>,
    pub completed_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheIterationWorkflowRemediationResult {
    pub completed: i64,
    pub failed: i64,
    pub cancelled: i64,
}

impl CacheIterationWorkflowRemediationResult {
    pub fn total(&self) -> i64 {
        self.completed + self.failed + self.cancelled
    }
}

#[async_trait::async_trait]
impl FindById for WorkflowCacheIterationRepository {
    async fn find_by_id<'e, E>(executor: E, id: Id) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS} \
             FROM workflow_cache_iteration WHERE id = $1"
        );
        sqlx::query_as::<_, WorkflowCacheIteration>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

impl WorkflowCacheIterationRepository {
    /// Finds terminal synthetic children whose completion message has not yet
    /// been reflected in the owning workflow's terminal task state.
    pub async fn find_stale_synthetic_completions(
        pool: &PgPool,
        grace_seconds: u64,
        limit: i64,
    ) -> Result<Vec<StaleSyntheticCacheIterationCompletion>> {
        let seconds = grace_seconds.min(i64::MAX as u64) as i64;
        let cutoff = Utc::now() - Duration::seconds(seconds);
        sqlx::query_as::<_, StaleSyntheticCacheIterationCompletion>(
            "SELECT e.id AS execution_id, e.action AS action_id, e.action_ref, e.status,
                    e.result, e.updated AS completed_at
             FROM execution e
             JOIN workflow_execution wf
               ON e.workflow_task->>'workflow_execution' = wf.id::TEXT
             WHERE e.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
               AND e.updated < $1
               AND e.workflow_task->>'task_name' IS NOT NULL
               AND e.workflow_task->>'task_index' IS NULL
               AND e.workflow_task->>'task_batch' = '0'
               AND wf.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
               AND NOT (e.workflow_task->>'task_name' = ANY(COALESCE(wf.completed_tasks, '{}'::TEXT[])))
               AND NOT (e.workflow_task->>'task_name' = ANY(COALESCE(wf.failed_tasks, '{}'::TEXT[])))
               AND NOT (e.workflow_task->>'task_name' = ANY(COALESCE(wf.skipped_tasks, '{}'::TEXT[])))
             ORDER BY e.updated, e.id
             LIMIT $2",
        )
        .bind(cutoff)
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    /// Releases scanning iteration pins after their owning workflow has
    /// already reached a terminal state.
    pub async fn remediate_scanning_for_terminal_workflows(
        pool: &PgPool,
        limit: i64,
    ) -> Result<CacheIterationWorkflowRemediationResult> {
        let states = sqlx::query_scalar::<_, WorkflowCacheIterationState>(
            "WITH candidate AS (
                 SELECT iteration.id, wf.status
                 FROM workflow_cache_iteration iteration
                 JOIN workflow_execution wf ON wf.id = iteration.workflow_execution
                 WHERE iteration.state = 'scanning'
                   AND wf.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')
                 ORDER BY iteration.updated, iteration.id
                 LIMIT $1
                 FOR UPDATE OF iteration SKIP LOCKED
             )
             UPDATE workflow_cache_iteration iteration
             SET state = CASE
                     WHEN candidate.status = 'completed' THEN 'completed'::workflow_cache_iteration_state_enum
                     WHEN candidate.status = 'cancelled' THEN 'cancelled'::workflow_cache_iteration_state_enum
                     ELSE 'failed'::workflow_cache_iteration_state_enum
                 END,
                 completed_at = NOW(),
                 error_summary = CASE
                     WHEN candidate.status = 'completed' THEN NULL
                     ELSE 'Supervisor synchronized cache iteration from terminal workflow status: '
                          || candidate.status::TEXT
                 END
             FROM candidate
             WHERE iteration.id = candidate.id
               AND iteration.state = 'scanning'
             RETURNING iteration.state",
        )
        .bind(limit.max(1))
        .fetch_all(pool)
        .await?;

        let mut result = CacheIterationWorkflowRemediationResult::default();
        for state in states {
            match state {
                WorkflowCacheIterationState::Completed => result.completed += 1,
                WorkflowCacheIterationState::Failed => result.failed += 1,
                WorkflowCacheIterationState::Cancelled => result.cancelled += 1,
                WorkflowCacheIterationState::Scanning => {}
            }
        }
        Ok(result)
    }

    /// Lists durable cache iterations belonging to a top-level execution.
    pub async fn list_by_execution<'e, E>(
        executor: E,
        execution: Id,
    ) -> Result<Vec<WorkflowCacheIteration>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS} \
             FROM workflow_cache_iteration \
             WHERE workflow_execution = (SELECT id FROM workflow_execution WHERE execution = $1) \
             ORDER BY created, id"
        );
        sqlx::query_as::<_, WorkflowCacheIteration>(&query)
            .bind(execution)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Creates an iteration or returns and locks the existing task iteration.
    /// The unique workflow/task key makes scheduler replay and concurrent entry
    /// dispatch converge on one durable cursor.
    pub async fn create_or_find_for_update(
        conn: &mut sqlx::PgConnection,
        input: CreateWorkflowCacheIterationInput,
    ) -> Result<WorkflowCacheIteration> {
        sqlx::query(
            "INSERT INTO workflow_cache_iteration \
             (workflow_execution, task_name, namespace, generation, page_size, batch_size, concurrency) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (workflow_execution, task_name) DO NOTHING",
        )
        .bind(input.workflow_execution)
        .bind(&input.task_name)
        .bind(input.namespace)
        .bind(input.generation)
        .bind(input.page_size)
        .bind(input.batch_size)
        .bind(input.concurrency)
        .execute(&mut *conn)
        .await?;

        Self::find_by_workflow_task_for_update(conn, input.workflow_execution, &input.task_name)
            .await?
            .ok_or_else(|| {
                Error::invalid_state("workflow cache iteration disappeared after create")
            })
    }

    pub async fn create<'e, E>(
        executor: E,
        input: CreateWorkflowCacheIterationInput,
    ) -> Result<WorkflowCacheIteration>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "INSERT INTO workflow_cache_iteration \
             (workflow_execution, task_name, namespace, generation, page_size, batch_size, concurrency) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING {WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS}"
        );
        sqlx::query_as::<_, WorkflowCacheIteration>(&query)
            .bind(input.workflow_execution)
            .bind(input.task_name)
            .bind(input.namespace)
            .bind(input.generation)
            .bind(input.page_size)
            .bind(input.batch_size)
            .bind(input.concurrency)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_workflow_task<'e, E>(
        executor: E,
        workflow_execution: Id,
        task_name: &str,
    ) -> Result<Option<WorkflowCacheIteration>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS} \
             FROM workflow_cache_iteration \
             WHERE workflow_execution = $1 AND task_name = $2"
        );
        sqlx::query_as::<_, WorkflowCacheIteration>(&query)
            .bind(workflow_execution)
            .bind(task_name)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_id_for_update<'e, E>(
        executor: E,
        id: Id,
    ) -> Result<Option<WorkflowCacheIteration>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS} \
             FROM workflow_cache_iteration WHERE id = $1 FOR UPDATE"
        );
        sqlx::query_as::<_, WorkflowCacheIteration>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_workflow_task_for_update<'e, E>(
        executor: E,
        workflow_execution: Id,
        task_name: &str,
    ) -> Result<Option<WorkflowCacheIteration>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS} \
             FROM workflow_cache_iteration \
             WHERE workflow_execution = $1 AND task_name = $2 FOR UPDATE"
        );
        sqlx::query_as::<_, WorkflowCacheIteration>(&query)
            .bind(workflow_execution)
            .bind(task_name)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Advances a scan monotonically. A stale retry or terminal iteration is not updated.
    pub async fn update_scan_progress<'e, E>(
        executor: E,
        id: Id,
        input: UpdateWorkflowCacheIterationProgressInput,
    ) -> Result<Option<WorkflowCacheIteration>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "UPDATE workflow_cache_iteration \
             SET last_external_id = $2, next_batch_index = $3, scanned_count = $4, \
                 dispatched_count = $5 \
             WHERE id = $1 AND state = 'scanning' \
               AND (last_external_id IS NULL OR last_external_id <= $2 COLLATE \"C\") \
               AND next_batch_index <= $3 AND scanned_count <= $4 AND dispatched_count <= $5 \
             RETURNING {WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS}"
        );
        sqlx::query_as::<_, WorkflowCacheIteration>(&query)
            .bind(id)
            .bind(input.last_external_id)
            .bind(input.next_batch_index)
            .bind(input.scanned_count)
            .bind(input.dispatched_count)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn mark_terminal<'e, E>(
        executor: E,
        id: Id,
        state: WorkflowCacheIterationState,
        error_summary: Option<&str>,
    ) -> Result<Option<WorkflowCacheIteration>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if !state.is_terminal() {
            return Err(Error::invalid_state(
                "workflow cache iteration terminal state is required",
            ));
        }
        if state == WorkflowCacheIterationState::Completed && error_summary.is_some() {
            return Err(Error::invalid_state(
                "completed workflow cache iterations cannot have an error summary",
            ));
        }

        let query = format!(
            "UPDATE workflow_cache_iteration \
             SET state = $2, completed_at = NOW(), error_summary = $3 \
             WHERE id = $1 AND state = 'scanning' \
             RETURNING {WORKFLOW_CACHE_ITERATION_SELECT_COLUMNS}"
        );
        sqlx::query_as::<_, WorkflowCacheIteration>(&query)
            .bind(id)
            .bind(state)
            .bind(error_summary)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use crate::models::WorkflowCacheIterationState;

    #[test]
    fn only_scanning_is_nonterminal() {
        assert!(!WorkflowCacheIterationState::Scanning.is_terminal());
        assert!(WorkflowCacheIterationState::Completed.is_terminal());
        assert!(WorkflowCacheIterationState::Failed.is_terminal());
        assert!(WorkflowCacheIterationState::Cancelled.is_terminal());
    }
}
