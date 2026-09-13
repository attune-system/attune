use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::log_stream::WorkflowLogOutboxRecord;
use crate::{Error, Result};

pub struct WorkflowLogOutboxRepository;

impl WorkflowLogOutboxRepository {
    pub async fn enqueue_append(
        connection: &mut PgConnection,
        workflow_execution: i64,
        payload: &[u8],
    ) -> Result<bool> {
        if payload.is_empty() {
            return Err(Error::validation("workflow log payload cannot be empty"));
        }
        Self::lock_stream(connection, workflow_execution).await?;
        if Self::has_seal(connection, workflow_execution).await? {
            return Ok(false);
        }
        let sequence = Self::next_sequence(connection, workflow_execution).await?;
        sqlx::query(
            "INSERT INTO workflow_log_outbox (workflow_execution, sequence, kind, payload) \
             VALUES ($1, $2, 'append', $3)",
        )
        .bind(workflow_execution)
        .bind(sequence)
        .bind(payload)
        .execute(connection)
        .await?;
        Ok(true)
    }

    pub async fn enqueue_seal(
        connection: &mut PgConnection,
        workflow_execution: i64,
    ) -> Result<bool> {
        Self::lock_stream(connection, workflow_execution).await?;
        if Self::has_seal(connection, workflow_execution).await? {
            return Ok(false);
        }
        let sequence = Self::next_sequence(connection, workflow_execution).await?;
        sqlx::query(
            "INSERT INTO workflow_log_outbox (workflow_execution, sequence, kind) \
             VALUES ($1, $2, 'seal')",
        )
        .bind(workflow_execution)
        .bind(sequence)
        .execute(connection)
        .await?;
        Ok(true)
    }

    async fn lock_stream(connection: &mut PgConnection, workflow_execution: i64) -> Result<()> {
        let locked: Option<i64> =
            sqlx::query_scalar("SELECT id FROM workflow_execution WHERE id = $1 FOR UPDATE")
                .bind(workflow_execution)
                .fetch_optional(connection)
                .await?;
        if locked.is_none() {
            return Err(Error::not_found(
                "workflow_execution",
                "id",
                workflow_execution.to_string(),
            ));
        }
        Ok(())
    }

    async fn has_seal(connection: &mut PgConnection, workflow_execution: i64) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_log_outbox \
             WHERE workflow_execution = $1 AND kind = 'seal')",
        )
        .bind(workflow_execution)
        .fetch_one(connection)
        .await
        .map_err(Into::into)
    }

    async fn next_sequence(connection: &mut PgConnection, workflow_execution: i64) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), -1) + 1 FROM workflow_log_outbox \
             WHERE workflow_execution = $1",
        )
        .bind(workflow_execution)
        .fetch_one(connection)
        .await
        .map_err(Into::into)
    }

    pub async fn claim_next(
        pool: &PgPool,
        owner: Uuid,
        lease: Duration,
    ) -> Result<Option<WorkflowLogOutboxRecord>> {
        let lease_milliseconds = i64::try_from(lease.as_millis())
            .map_err(|_| Error::validation("workflow log claim lease is too large"))?;
        sqlx::query_as(
            "WITH candidate AS ( \
                 SELECT item.id \
                 FROM workflow_log_outbox item \
                 WHERE item.delivered_at IS NULL \
                   AND item.available_at <= clock_timestamp() \
                   AND (item.claim_expires_at IS NULL OR item.claim_expires_at <= clock_timestamp()) \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM workflow_log_outbox prior \
                       WHERE prior.workflow_execution = item.workflow_execution \
                         AND prior.sequence < item.sequence \
                         AND prior.delivered_at IS NULL \
                   ) \
                 ORDER BY item.available_at, item.created, item.id \
                 FOR UPDATE SKIP LOCKED \
                 LIMIT 1 \
             ), claimed AS ( \
                 UPDATE workflow_log_outbox item \
                 SET claimed_by = $1, \
                     claim_expires_at = clock_timestamp() + ($2 * INTERVAL '1 millisecond'), \
                     attempt_count = attempt_count + 1 \
                 FROM candidate \
                 WHERE item.id = candidate.id \
                 RETURNING item.id, item.workflow_execution, item.sequence, item.kind, \
                           item.payload, item.claimed_by, item.attempt_count \
             ) \
             SELECT claimed.id, claimed.workflow_execution, claimed.sequence, claimed.kind, \
                    claimed.payload, workflow.execution AS parent_execution, \
                    execution.action_ref, claimed.claimed_by, claimed.attempt_count \
             FROM claimed \
             JOIN workflow_execution workflow ON workflow.id = claimed.workflow_execution \
             JOIN execution ON execution.id = workflow.execution",
        )
        .bind(owner)
        .bind(lease_milliseconds)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn mark_delivered(pool: &PgPool, id: i64, owner: Uuid) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE workflow_log_outbox \
             SET delivered_at = NOW(), payload = NULL, claimed_by = NULL, \
                 claim_expires_at = NULL, last_error = NULL \
             WHERE id = $1 AND claimed_by = $2 AND delivered_at IS NULL",
        )
        .bind(id)
        .bind(owner)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn release_after_failure(
        pool: &PgPool,
        id: i64,
        owner: Uuid,
        retry_at: DateTime<Utc>,
        sanitized_error: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE workflow_log_outbox \
             SET available_at = $3, claimed_by = NULL, claim_expires_at = NULL, last_error = $4 \
             WHERE id = $1 AND claimed_by = $2 AND delivered_at IS NULL",
        )
        .bind(id)
        .bind(owner)
        .bind(retry_at)
        .bind(sanitized_error)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}
