use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::log_stream::WorkflowLogOutboxRecord;
use crate::{Error, Result};

pub struct WorkflowLogOutboxRepository;

impl WorkflowLogOutboxRepository {
    pub async fn effective_segment_max_bytes(
        connection: &mut PgConnection,
        workflow_execution: i64,
        configured_max_bytes: usize,
    ) -> Result<usize> {
        if configured_max_bytes == 0 {
            return Err(Error::validation(
                "workflow log segment size must be greater than zero",
            ));
        }
        let persisted: Option<i64> = sqlx::query_scalar(
            "SELECT stream.max_unflushed_bytes \
             FROM workflow_execution workflow \
             JOIN execution ON execution.id = workflow.execution \
             JOIN artifact ON artifact.ref = execution.action_ref || '.workflow.log' \
             JOIN artifact_version version \
               ON version.artifact = artifact.id AND version.execution = execution.id \
             JOIN log_stream stream ON stream.artifact_version = version.id \
             WHERE workflow.id = $1 \
             ORDER BY version.version DESC LIMIT 1",
        )
        .bind(workflow_execution)
        .fetch_optional(connection)
        .await?;
        let Some(persisted) = persisted else {
            return Ok(configured_max_bytes);
        };
        let persisted = usize::try_from(persisted)
            .map_err(|_| Error::invalid_state("persisted workflow log segment size is invalid"))?;
        if persisted == 0 {
            return Err(Error::invalid_state(
                "persisted workflow log segment size is zero",
            ));
        }
        Ok(configured_max_bytes.min(persisted))
    }

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
            "INSERT INTO workflow_log_outbox \
                 (workflow_execution, sequence, kind, payload, is_head) \
             SELECT $1, $2, 'append', $3, NOT EXISTS ( \
                 SELECT 1 FROM workflow_log_outbox \
                 WHERE workflow_execution = $1 AND delivered_at IS NULL \
             )",
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
            "INSERT INTO workflow_log_outbox (workflow_execution, sequence, kind, is_head) \
             SELECT $1, $2, 'seal', NOT EXISTS ( \
                 SELECT 1 FROM workflow_log_outbox \
                 WHERE workflow_execution = $1 AND delivered_at IS NULL \
             )",
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
                 WHERE item.is_head \
                   AND item.delivered_at IS NULL \
                   AND item.failed_at IS NULL \
                   AND item.available_at <= clock_timestamp() \
                   AND (item.claim_expires_at IS NULL OR item.claim_expires_at <= clock_timestamp()) \
                 ORDER BY item.available_at, item.created, item.id \
                 FOR UPDATE OF item SKIP LOCKED \
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

    pub async fn assign_delivery_sequence(
        pool: &PgPool,
        id: i64,
        owner: Uuid,
        stream_next_sequence: i64,
    ) -> Result<i64> {
        let sequence: Option<i64> = sqlx::query_scalar(
            "UPDATE workflow_log_outbox \
             SET delivery_sequence = COALESCE(delivery_sequence, $3) \
             WHERE id = $1 AND claimed_by = $2 AND delivered_at IS NULL AND failed_at IS NULL \
             RETURNING delivery_sequence",
        )
        .bind(id)
        .bind(owner)
        .bind(stream_next_sequence)
        .fetch_optional(pool)
        .await?;
        sequence.ok_or_else(|| Error::invalid_state("workflow log claim is no longer active"))
    }

    pub async fn mark_delivered(pool: &PgPool, id: i64, owner: Uuid) -> Result<bool> {
        let mut transaction = pool.begin().await?;
        sqlx::query(
            "SELECT workflow.id \
             FROM workflow_log_outbox outbox \
             JOIN workflow_execution workflow ON workflow.id = outbox.workflow_execution \
             WHERE outbox.id = $1 FOR UPDATE OF workflow",
        )
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        let delivered: bool = sqlx::query_scalar(
            "WITH delivered AS ( \
                 UPDATE workflow_log_outbox \
                 SET delivered_at = NOW(), payload = NULL, claimed_by = NULL, \
                     claim_expires_at = NULL, last_error = NULL, is_head = FALSE \
                 WHERE id = $1 AND claimed_by = $2 AND delivered_at IS NULL \
                   AND failed_at IS NULL AND is_head \
                 RETURNING workflow_execution \
             ), promoted AS ( \
                 UPDATE workflow_log_outbox next \
                 SET is_head = TRUE \
                 FROM delivered \
                 WHERE next.id = ( \
                     SELECT candidate.id FROM workflow_log_outbox candidate \
                     WHERE candidate.workflow_execution = delivered.workflow_execution \
                       AND candidate.id <> $1 \
                       AND candidate.delivered_at IS NULL \
                     ORDER BY candidate.sequence LIMIT 1 \
                 ) \
                 RETURNING next.id \
             ) \
             SELECT EXISTS(SELECT 1 FROM delivered) \
                    OR EXISTS(SELECT 1 FROM promoted)",
        )
        .bind(id)
        .bind(owner)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(delivered)
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
              WHERE id = $1 AND claimed_by = $2 AND delivered_at IS NULL AND failed_at IS NULL",
        )
        .bind(id)
        .bind(owner)
        .bind(retry_at)
        .bind(sanitized_error)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_permanently_failed(
        pool: &PgPool,
        id: i64,
        owner: Uuid,
        sanitized_error: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE workflow_log_outbox \
             SET failed_at = NOW(), claimed_by = NULL, claim_expires_at = NULL, last_error = $3 \
             WHERE id = $1 AND claimed_by = $2 AND delivered_at IS NULL AND failed_at IS NULL",
        )
        .bind(id)
        .bind(owner)
        .bind(sanitized_error)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Requeue a permanently failed row without changing its transport identity.
    pub async fn retry_failed(pool: &PgPool, id: i64) -> Result<bool> {
        let mut transaction = pool.begin().await?;
        let workflow_execution: Option<i64> = sqlx::query_scalar(
            "UPDATE workflow_log_outbox \
             SET failed_at = NULL, available_at = NOW(), attempt_count = 0, last_error = NULL \
             WHERE id = $1 AND failed_at IS NOT NULL AND delivered_at IS NULL \
             RETURNING workflow_execution",
        )
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(workflow_execution) = workflow_execution else {
            transaction.rollback().await?;
            return Ok(false);
        };
        sqlx::query(
            "UPDATE artifact_version version \
             SET meta = (COALESCE(version.meta, '{}'::jsonb) - 'log_failure') \
                        || jsonb_build_object('log_state', 'pending') \
             FROM artifact, execution, workflow_execution workflow \
             WHERE workflow.id = $1 \
               AND execution.id = workflow.execution \
               AND artifact.ref = execution.action_ref || '.workflow.log' \
               AND version.artifact = artifact.id \
               AND version.execution = execution.id \
               AND version.meta->>'log_state' = 'degraded'",
        )
        .bind(workflow_execution)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }
}
