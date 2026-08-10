//! Entity history repository for querying TimescaleDB history hypertables
//!
//! This module provides read-only query methods for the `<entity>_history` tables.
//! History records are written exclusively by PostgreSQL triggers — this repository
//! only reads them.

use chrono::{DateTime, Utc};
use sqlx::{Executor, Postgres, QueryBuilder};

use crate::models::entity_history::{EntityHistoryRecord, HistoryEntityType};
use crate::Result;

/// Repository for querying entity history hypertables.
///
/// All methods are read-only. History records are populated by PostgreSQL
/// `AFTER INSERT OR UPDATE OR DELETE` triggers on the operational tables.
pub struct EntityHistoryRepository;

/// Query parameters for filtering history records.
#[derive(Debug, Clone, Default)]
pub struct HistoryQueryParams {
    /// Filter by entity ID (e.g., execution.id)
    pub entity_id: Option<i64>,

    /// Filter by entity ref (e.g., action_ref, worker name)
    pub entity_ref: Option<String>,

    /// Filter by operation type: `INSERT`, `UPDATE`, or `DELETE`
    pub operation: Option<String>,

    /// Only include records where this field was changed
    pub changed_field: Option<String>,

    /// Only include records at or after this time
    pub since: Option<DateTime<Utc>>,

    /// Only include records at or before this time
    pub until: Option<DateTime<Utc>>,

    /// Maximum number of records to return (default: 100, max: 1000)
    pub limit: Option<i64>,

    /// Offset for pagination
    pub offset: Option<i64>,
}

impl HistoryQueryParams {
    /// Returns the effective limit, capped at 1000.
    pub fn effective_limit(&self) -> i64 {
        self.limit.unwrap_or(100).clamp(1, 1000)
    }

    /// Returns the effective offset.
    pub fn effective_offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }
}

impl EntityHistoryRepository {
    /// Query history records for a given entity type with optional filters.
    ///
    /// Results are ordered newest first with stable tie-breakers.
    pub async fn query<'e, E>(
        executor: E,
        entity_type: HistoryEntityType,
        params: &HistoryQueryParams,
    ) -> Result<Vec<EntityHistoryRecord>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // We must use format! for the table name since it can't be a bind parameter,
        // but HistoryEntityType::table_name() returns a known static str so this is safe.
        let table = entity_type.table_name();

        let mut qb: QueryBuilder<Postgres> =
            QueryBuilder::new(format!("SELECT time, operation, entity_id, entity_ref, changed_fields, old_values, new_values FROM {table} WHERE 1=1"));

        if let Some(entity_id) = params.entity_id {
            qb.push(" AND entity_id = ").push_bind(entity_id);
        }

        if let Some(ref entity_ref) = params.entity_ref {
            qb.push(" AND entity_ref = ").push_bind(entity_ref.clone());
        }

        if let Some(ref operation) = params.operation {
            qb.push(" AND operation = ")
                .push_bind(operation.to_uppercase());
        }

        if let Some(ref changed_field) = params.changed_field {
            qb.push(" AND ")
                .push_bind(changed_field.clone())
                .push(" = ANY(changed_fields)");
        }

        if let Some(since) = params.since {
            qb.push(" AND time >= ").push_bind(since);
        }

        if let Some(until) = params.until {
            qb.push(" AND time <= ").push_bind(until);
        }

        qb.push(
            " ORDER BY time DESC, entity_id DESC, operation ASC, entity_ref ASC, changed_fields::text ASC",
        );
        qb.push(" LIMIT ").push_bind(params.effective_limit());
        qb.push(" OFFSET ").push_bind(params.effective_offset());

        let records = qb
            .build_query_as::<EntityHistoryRecord>()
            .fetch_all(executor)
            .await?;

        Ok(records)
    }

    /// Count history records for a given entity type with optional filters.
    ///
    /// Useful for pagination metadata.
    pub async fn count<'e, E>(
        executor: E,
        entity_type: HistoryEntityType,
        params: &HistoryQueryParams,
    ) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let table = entity_type.table_name();

        let mut qb: QueryBuilder<Postgres> =
            QueryBuilder::new(format!("SELECT COUNT(*) FROM {table} WHERE 1=1"));

        if let Some(entity_id) = params.entity_id {
            qb.push(" AND entity_id = ").push_bind(entity_id);
        }

        if let Some(ref entity_ref) = params.entity_ref {
            qb.push(" AND entity_ref = ").push_bind(entity_ref.clone());
        }

        if let Some(ref operation) = params.operation {
            qb.push(" AND operation = ")
                .push_bind(operation.to_uppercase());
        }

        if let Some(ref changed_field) = params.changed_field {
            qb.push(" AND ")
                .push_bind(changed_field.clone())
                .push(" = ANY(changed_fields)");
        }

        if let Some(since) = params.since {
            qb.push(" AND time >= ").push_bind(since);
        }

        if let Some(until) = params.until {
            qb.push(" AND time <= ").push_bind(until);
        }

        let row: (i64,) = qb.build_query_as().fetch_one(executor).await?;

        Ok(row.0)
    }

    /// Get history records for a specific entity by ID.
    ///
    /// Convenience method equivalent to `query()` with `entity_id` set.
    pub async fn find_by_entity_id<'e, E>(
        executor: E,
        entity_type: HistoryEntityType,
        entity_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<EntityHistoryRecord>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let params = HistoryQueryParams {
            entity_id: Some(entity_id),
            limit,
            ..Default::default()
        };
        Self::query(executor, entity_type, &params).await
    }

    /// Get only status-change history records for a specific entity.
    ///
    /// Filters to UPDATE operations where `changed_fields` includes `"status"`.
    pub async fn find_status_changes<'e, E>(
        executor: E,
        entity_type: HistoryEntityType,
        entity_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<EntityHistoryRecord>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let params = HistoryQueryParams {
            entity_id: Some(entity_id),
            operation: Some("UPDATE".to_string()),
            changed_field: Some("status".to_string()),
            limit,
            ..Default::default()
        };
        Self::query(executor, entity_type, &params).await
    }

    /// Get the most recent history record for a specific entity.
    pub async fn find_latest<'e, E>(
        executor: E,
        entity_type: HistoryEntityType,
        entity_id: i64,
    ) -> Result<Option<EntityHistoryRecord>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let records = Self::find_by_entity_id(executor, entity_type, entity_id, Some(1)).await?;
        Ok(records.into_iter().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_query_params_defaults() {
        let params = HistoryQueryParams::default();
        assert_eq!(params.effective_limit(), 100);
        assert_eq!(params.effective_offset(), 0);
    }

    #[test]
    fn test_history_query_params_limit_cap() {
        let params = HistoryQueryParams {
            limit: Some(5000),
            ..Default::default()
        };
        assert_eq!(params.effective_limit(), 1000);
    }

    #[test]
    fn test_history_query_params_limit_min() {
        let params = HistoryQueryParams {
            limit: Some(-10),
            ..Default::default()
        };
        assert_eq!(params.effective_limit(), 1);
    }

    #[test]
    fn test_history_query_params_offset_min() {
        let params = HistoryQueryParams {
            offset: Some(-5),
            ..Default::default()
        };
        assert_eq!(params.effective_offset(), 0);
    }

    #[test]
    fn test_history_entity_type_table_name() {
        assert_eq!(
            HistoryEntityType::Execution.table_name(),
            "execution_history"
        );
        assert_eq!(HistoryEntityType::Worker.table_name(), "worker_history");
    }

    #[test]
    fn test_history_entity_type_from_str() {
        assert_eq!(
            "execution".parse::<HistoryEntityType>().unwrap(),
            HistoryEntityType::Execution
        );
        assert_eq!(
            "Worker".parse::<HistoryEntityType>().unwrap(),
            HistoryEntityType::Worker
        );
        assert!("enforcement".parse::<HistoryEntityType>().is_err());
        assert!("event".parse::<HistoryEntityType>().is_err());
        assert!("unknown".parse::<HistoryEntityType>().is_err());
    }

    #[test]
    fn test_history_entity_type_display() {
        assert_eq!(HistoryEntityType::Execution.to_string(), "execution");
        assert_eq!(HistoryEntityType::Worker.to_string(), "worker");
    }
}
