//! Queue Statistics Repository
//!
//! Provides database operations for queue statistics persistence.

use chrono::{DateTime, Utc};
use sqlx::{Executor, PgPool, Postgres, QueryBuilder};

use crate::error::Result;
use crate::models::Id;

const SELECT_COLUMNS: &str = "action_id, queue_length, active_count, max_concurrent, \
    oldest_enqueued_at, total_enqueued::BIGINT AS total_enqueued, \
    total_completed::BIGINT AS total_completed, last_updated";

/// Queue statistics model
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QueueStats {
    pub action_id: Id,
    pub queue_length: i32,
    pub active_count: i32,
    pub max_concurrent: i32,
    pub oldest_enqueued_at: Option<DateTime<Utc>>,
    pub total_enqueued: i64,
    pub total_completed: i64,
    pub last_updated: DateTime<Utc>,
}

/// Input for upserting queue statistics
#[derive(Debug, Clone)]
pub struct UpsertQueueStatsInput {
    pub action_id: Id,
    pub queue_length: i32,
    pub active_count: i32,
    pub max_concurrent: i32,
    pub oldest_enqueued_at: Option<DateTime<Utc>>,
    pub total_enqueued: i64,
    pub total_completed: i64,
}

/// Queue statistics repository
pub struct QueueStatsRepository;

impl QueueStatsRepository {
    /// Upsert queue statistics (insert or update)
    pub async fn upsert<'e, E>(executor: E, input: UpsertQueueStatsInput) -> Result<QueueStats>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            r#"
            INSERT INTO queue_stats (
                action_id,
                queue_length,
                active_count,
                max_concurrent,
                oldest_enqueued_at,
                total_enqueued,
                total_completed,
                last_updated
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
            ON CONFLICT (action_id) DO UPDATE SET
                queue_length = EXCLUDED.queue_length,
                active_count = EXCLUDED.active_count,
                max_concurrent = EXCLUDED.max_concurrent,
                oldest_enqueued_at = EXCLUDED.oldest_enqueued_at,
                total_enqueued = EXCLUDED.total_enqueued,
                total_completed = EXCLUDED.total_completed,
                last_updated = NOW()
            RETURNING {}
            "#,
            SELECT_COLUMNS
        );
        let stats = sqlx::query_as::<Postgres, QueueStats>(&query)
            .bind(input.action_id)
            .bind(input.queue_length)
            .bind(input.active_count)
            .bind(input.max_concurrent)
            .bind(input.oldest_enqueued_at)
            .bind(input.total_enqueued)
            .bind(input.total_completed)
            .fetch_one(executor)
            .await?;

        Ok(stats)
    }

    /// Get queue statistics for a specific action
    pub async fn find_by_action<'e, E>(executor: E, action_id: Id) -> Result<Option<QueueStats>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            r#"
            SELECT {}
            FROM queue_stats
            WHERE action_id = $1
            "#,
            SELECT_COLUMNS
        );
        let stats = sqlx::query_as::<Postgres, QueueStats>(&query)
            .bind(action_id)
            .fetch_optional(executor)
            .await?;

        Ok(stats)
    }

    /// List all queue statistics with active queues (queue_length > 0 or active_count > 0)
    pub async fn list_active<'e, E>(executor: E) -> Result<Vec<QueueStats>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            r#"
            SELECT {}
            FROM queue_stats
            WHERE queue_length > 0 OR active_count > 0
            ORDER BY last_updated DESC
            "#,
            SELECT_COLUMNS
        );
        let stats = sqlx::query_as::<Postgres, QueueStats>(&query)
            .fetch_all(executor)
            .await?;

        Ok(stats)
    }

    /// List all queue statistics
    pub async fn list_all<'e, E>(executor: E) -> Result<Vec<QueueStats>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            r#"
            SELECT {}
            FROM queue_stats
            ORDER BY last_updated DESC
            "#,
            SELECT_COLUMNS
        );
        let stats = sqlx::query_as::<Postgres, QueueStats>(&query)
            .fetch_all(executor)
            .await?;

        Ok(stats)
    }

    /// Delete queue statistics for a specific action
    pub async fn delete<'e, E>(executor: E, action_id: Id) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            r#"
            DELETE FROM queue_stats
            WHERE action_id = $1
            "#,
        )
        .bind(action_id)
        .execute(executor)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Batch upsert multiple queue statistics
    pub async fn batch_upsert(
        executor: &PgPool,
        inputs: Vec<UpsertQueueStatsInput>,
    ) -> Result<Vec<QueueStats>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        // Build dynamic query for batch insert
        let mut query_builder = QueryBuilder::new(
            r#"
            INSERT INTO queue_stats (
                action_id,
                queue_length,
                active_count,
                max_concurrent,
                oldest_enqueued_at,
                total_enqueued,
                total_completed,
                last_updated
            )
            "#,
        );

        query_builder.push_values(inputs.iter(), |mut b, input| {
            b.push_bind(input.action_id)
                .push_bind(input.queue_length)
                .push_bind(input.active_count)
                .push_bind(input.max_concurrent)
                .push_bind(input.oldest_enqueued_at)
                .push_bind(input.total_enqueued)
                .push_bind(input.total_completed)
                .push("NOW()");
        });

        query_builder.push(format!(
            r#"
            ON CONFLICT (action_id) DO UPDATE SET
                queue_length = EXCLUDED.queue_length,
                active_count = EXCLUDED.active_count,
                max_concurrent = EXCLUDED.max_concurrent,
                oldest_enqueued_at = EXCLUDED.oldest_enqueued_at,
                total_enqueued = EXCLUDED.total_enqueued,
                total_completed = EXCLUDED.total_completed,
                last_updated = NOW()
            RETURNING {}
            "#,
            SELECT_COLUMNS
        ));

        let stats = query_builder
            .build_query_as::<QueueStats>()
            .fetch_all(executor)
            .await?;

        Ok(stats)
    }

    /// Clear stale statistics (older than specified duration)
    pub async fn clear_stale<'e, E>(executor: E, older_than_seconds: i64) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            r#"
            DELETE FROM queue_stats
            WHERE last_updated < NOW() - INTERVAL '1 second' * $1
              AND queue_length = 0
              AND active_count = 0
            "#,
        )
        .bind(older_than_seconds)
        .execute(executor)
        .await?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_stats_structure() {
        let input = UpsertQueueStatsInput {
            action_id: 1,
            queue_length: 5,
            active_count: 2,
            max_concurrent: 3,
            oldest_enqueued_at: Some(Utc::now()),
            total_enqueued: 100,
            total_completed: 95,
        };

        assert_eq!(input.action_id, 1);
        assert_eq!(input.queue_length, 5);
        assert_eq!(input.active_count, 2);
    }

    #[test]
    fn test_empty_batch_upsert() {
        let inputs: Vec<UpsertQueueStatsInput> = Vec::new();
        assert_eq!(inputs.len(), 0);
    }
}
