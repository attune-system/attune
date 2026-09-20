//! Durable workflow task prerequisites.

use crate::{
    models::{
        enums::WorkflowTaskWaitState,
        workflow::{WorkflowTaskWait, WorkflowTaskWaitTarget, WORKFLOW_TASK_WAIT_SELECT_COLUMNS},
        Id,
    },
    Error, Result,
};
use serde_json::Value as JsonValue;
use sqlx::{Executor, PgConnection, Postgres};

pub struct WorkflowTaskWaitRepository;

#[derive(Debug, Clone)]
pub struct CreateWorkflowTaskWaitInput {
    pub workflow_execution: Id,
    pub task_name: String,
    pub target: WorkflowTaskWaitTarget,
}

impl WorkflowTaskWaitRepository {
    pub async fn create_or_get(
        conn: &mut PgConnection,
        input: CreateWorkflowTaskWaitInput,
    ) -> Result<WorkflowTaskWait> {
        let (inquiry, target_execution, work_queue_item) = match input.target {
            WorkflowTaskWaitTarget::Inquiry(id) => (Some(id), None, None),
            WorkflowTaskWaitTarget::Execution(id) => (None, Some(id), None),
            WorkflowTaskWaitTarget::WorkQueueItem(id) => (None, None, Some(id)),
        };
        let query = format!(
            "INSERT INTO workflow_task_wait \
                (workflow_execution, task_name, kind, inquiry, target_execution, work_queue_item, active_work_queue_item) \
             VALUES ($1, $2, $3, $4, $5, $6, $6) \
             ON CONFLICT (workflow_execution, task_name) DO NOTHING \
             RETURNING {WORKFLOW_TASK_WAIT_SELECT_COLUMNS}"
        );
        if let Some(wait) = sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(input.workflow_execution)
            .bind(&input.task_name)
            .bind(input.target.kind())
            .bind(inquiry)
            .bind(target_execution)
            .bind(work_queue_item)
            .fetch_optional(&mut *conn)
            .await?
        {
            return Ok(wait);
        }

        let existing =
            Self::find_by_workflow_task(&mut *conn, input.workflow_execution, &input.task_name)
                .await?
                .ok_or_else(|| {
                    Error::InvalidState("workflow task wait disappeared after conflict".to_string())
                })?;
        if existing.target()? != input.target {
            return Err(Error::InvalidState(format!(
                "workflow task '{}' already waits on a different target",
                input.task_name
            )));
        }
        Ok(existing)
    }

    pub async fn find_by_workflow_task<'e, E>(
        executor: E,
        workflow_execution: Id,
        task_name: &str,
    ) -> Result<Option<WorkflowTaskWait>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_TASK_WAIT_SELECT_COLUMNS} FROM workflow_task_wait \
             WHERE workflow_execution = $1 AND task_name = $2"
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(workflow_execution)
            .bind(task_name)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_workflow_task_for_update(
        conn: &mut PgConnection,
        workflow_execution: Id,
        task_name: &str,
    ) -> Result<Option<WorkflowTaskWait>> {
        let query = format!(
            "SELECT {WORKFLOW_TASK_WAIT_SELECT_COLUMNS} FROM workflow_task_wait \
             WHERE workflow_execution = $1 AND task_name = $2 FOR UPDATE"
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(workflow_execution)
            .bind(task_name)
            .fetch_optional(conn)
            .await
            .map_err(Into::into)
    }

    pub async fn find_reconcilable_by_inquiry<'e, E>(
        executor: E,
        inquiry: Id,
    ) -> Result<Vec<WorkflowTaskWait>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_TASK_WAIT_SELECT_COLUMNS} FROM workflow_task_wait \
             WHERE inquiry = $1 \
               AND (state IN ($2, $3) OR (state IN ($4, $5) \
                    AND (result->>'_delivery_complete') IS DISTINCT FROM 'true')) \
             ORDER BY id"
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(inquiry)
            .bind(WorkflowTaskWaitState::Waiting)
            .bind(WorkflowTaskWaitState::Released)
            .bind(WorkflowTaskWaitState::TimedOut)
            .bind(WorkflowTaskWaitState::Failed)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_reconcilable_by_target<'e, E>(
        executor: E,
        target: WorkflowTaskWaitTarget,
    ) -> Result<Vec<WorkflowTaskWait>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let target_column = match target {
            WorkflowTaskWaitTarget::Inquiry(_) => "inquiry",
            WorkflowTaskWaitTarget::Execution(_) => "target_execution",
            WorkflowTaskWaitTarget::WorkQueueItem(_) => "work_queue_item",
        };
        let query = format!(
            "SELECT {WORKFLOW_TASK_WAIT_SELECT_COLUMNS} FROM workflow_task_wait \
             WHERE kind = $1 AND {target_column} = $2 \
               AND (state IN ($3, $4) OR (state IN ($5, $6) \
                    AND (result->>'_delivery_complete') IS DISTINCT FROM 'true')) \
             ORDER BY id"
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(target.kind())
            .bind(target.id())
            .bind(WorkflowTaskWaitState::Waiting)
            .bind(WorkflowTaskWaitState::Released)
            .bind(WorkflowTaskWaitState::TimedOut)
            .bind(WorkflowTaskWaitState::Failed)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_resolvable<'e, E>(executor: E, limit: i64) -> Result<Vec<WorkflowTaskWait>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM workflow_task_wait w \
             WHERE (w.state = 'waiting' AND ( \
                    (w.kind = 'inquiry' AND EXISTS ( \
                        SELECT 1 FROM inquiry i \
                        WHERE i.id = w.inquiry AND i.status <> 'pending' \
                    )) OR \
                    (w.kind = 'execution' AND (NOT EXISTS ( \
                        SELECT 1 FROM execution target WHERE target.id = w.target_execution \
                    ) OR EXISTS ( \
                        SELECT 1 FROM execution target WHERE target.id = w.target_execution \
                          AND target.status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned') \
                    ))) OR \
                    (w.kind = 'work_queue_item' AND (NOT EXISTS ( \
                        SELECT 1 FROM work_queue_item item WHERE item.id = w.work_queue_item \
                    ) OR EXISTS ( \
                        SELECT 1 FROM work_queue_item item WHERE item.id = w.work_queue_item \
                          AND item.status IN ('completed', 'failed', 'skipped', 'cancelled') \
                    ))) \
                 )) \
                  OR (w.state = 'released' AND EXISTS ( \
                     SELECT 1 FROM execution e \
                     WHERE e.workflow_task->>'workflow_execution' = w.workflow_execution::TEXT \
                        AND e.workflow_task->>'task_name' = w.task_name \
                        AND e.status = 'requested' \
                 )) \
                  OR (w.state IN ('timed_out', 'failed') \
                      AND (w.result->>'_delivery_complete') IS DISTINCT FROM 'true') \
             ORDER BY w.id LIMIT $1",
            qualified_select_columns("w")
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(limit)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn list_for_workflow<'e, E>(
        executor: E,
        workflow_execution: Id,
    ) -> Result<Vec<WorkflowTaskWait>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_TASK_WAIT_SELECT_COLUMNS} FROM workflow_task_wait \
             WHERE workflow_execution = $1 ORDER BY id"
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(workflow_execution)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Lists durable task waits belonging to a top-level execution.
    pub async fn list_by_execution<'e, E>(
        executor: E,
        execution: Id,
    ) -> Result<Vec<WorkflowTaskWait>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {WORKFLOW_TASK_WAIT_SELECT_COLUMNS} FROM workflow_task_wait \
             WHERE workflow_execution = (SELECT id FROM workflow_execution WHERE execution = $1) \
             ORDER BY created, id"
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(execution)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn transition_waiting<'e, E>(
        executor: E,
        id: Id,
        state: WorkflowTaskWaitState,
        result: Option<JsonValue>,
    ) -> Result<Option<WorkflowTaskWait>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if state == WorkflowTaskWaitState::Waiting {
            return Err(Error::InvalidState(
                "workflow task wait transition must be terminal".to_string(),
            ));
        }
        let query = format!(
            "UPDATE workflow_task_wait SET state = $2, result = $3, resolved_at = NOW(), \
             active_work_queue_item = NULL, \
             released_at = CASE WHEN $2 = 'released' THEN NOW() ELSE NULL END \
             WHERE id = $1 AND state = $4 RETURNING {WORKFLOW_TASK_WAIT_SELECT_COLUMNS}"
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(id)
            .bind(state)
            .bind(result)
            .bind(WorkflowTaskWaitState::Waiting)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn count_waiting<'e, E>(executor: E, workflow_execution: Id) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_task_wait WHERE workflow_execution = $1 AND state = $2",
        )
        .bind(workflow_execution)
        .bind(WorkflowTaskWaitState::Waiting)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn count_pending_terminal_delivery<'e, E>(
        executor: E,
        workflow_execution: Id,
    ) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_task_wait \
             WHERE workflow_execution = $1 AND state IN ($2, $3) \
               AND (result->>'_delivery_complete') IS DISTINCT FROM 'true'",
        )
        .bind(workflow_execution)
        .bind(WorkflowTaskWaitState::TimedOut)
        .bind(WorkflowTaskWaitState::Failed)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn mark_terminal_delivery_complete<'e, E>(executor: E, id: Id) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            "UPDATE workflow_task_wait \
             SET result = COALESCE(result, '{}'::JSONB) || '{\"_delivery_complete\": true}'::JSONB \
             WHERE id = $1 AND state IN ($2, $3) \
               AND (result->>'_delivery_complete') IS DISTINCT FROM 'true'",
        )
        .bind(id)
        .bind(WorkflowTaskWaitState::TimedOut)
        .bind(WorkflowTaskWaitState::Failed)
        .execute(executor)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn cancel_waiting_for_workflow<'e, E>(
        executor: E,
        workflow_execution: Id,
        result: JsonValue,
    ) -> Result<Vec<WorkflowTaskWait>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "UPDATE workflow_task_wait SET state = $2, result = $3, resolved_at = NOW(), \
             active_work_queue_item = NULL \
             WHERE workflow_execution = $1 AND state = $4 \
             RETURNING {WORKFLOW_TASK_WAIT_SELECT_COLUMNS}"
        );
        sqlx::query_as::<_, WorkflowTaskWait>(&query)
            .bind(workflow_execution)
            .bind(WorkflowTaskWaitState::Cancelled)
            .bind(result)
            .bind(WorkflowTaskWaitState::Waiting)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

fn qualified_select_columns(alias: &str) -> String {
    WORKFLOW_TASK_WAIT_SELECT_COLUMNS
        .split(", ")
        .map(|column| format!("{alias}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}
