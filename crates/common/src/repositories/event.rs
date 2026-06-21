//! Event and Enforcement repository for database operations
//!
//! This module provides CRUD operations and queries for Event and Enforcement entities.
//! Note: Events are immutable time-series data — there is no Update impl for EventRepository.

use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::models::{
    enums::{EnforcementCondition, EnforcementStatus},
    event::*,
    Id, JsonDict,
};
use crate::Result;
use sqlx::{Executor, Postgres, QueryBuilder, Row};

use super::{ref_filter_like_pattern, Create, Delete, FindById, List, Repository, Update};

// ============================================================================
// Event Search
// ============================================================================

/// Filters for [`EventRepository::search`].
///
/// All fields are optional. When set, the corresponding WHERE clause is added.
/// Pagination is always applied.
#[derive(Debug, Clone)]
pub struct EventSearchFilters {
    pub trigger: Option<Id>,
    pub trigger_ref: Option<String>,
    pub source: Option<Id>,
    pub rule_ref: Option<String>,
    pub trace_tag: Option<String>,
    pub include_total: bool,
    pub limit: u32,
    pub offset: u32,
}

impl Default for EventSearchFilters {
    fn default() -> Self {
        Self {
            trigger: None,
            trigger_ref: None,
            source: None,
            rule_ref: None,
            trace_tag: None,
            include_total: true,
            limit: 0,
            offset: 0,
        }
    }
}

/// Result of [`EventRepository::search`].
#[derive(Debug)]
pub struct EventSearchResult {
    pub rows: Vec<Event>,
    pub total: Option<u64>,
    pub has_next: bool,
}

// ============================================================================
// Enforcement Search
// ============================================================================

/// Filters for [`EnforcementRepository::search`].
///
/// All fields are optional and combinable. Pagination is always applied.
#[derive(Debug, Clone)]
pub struct EnforcementSearchFilters {
    pub rule: Option<Id>,
    pub event: Option<Id>,
    pub status: Option<EnforcementStatus>,
    pub trigger_ref: Option<String>,
    pub rule_ref: Option<String>,
    pub trace_tag: Option<String>,
    pub include_total: bool,
    pub limit: u32,
    pub offset: u32,
}

impl Default for EnforcementSearchFilters {
    fn default() -> Self {
        Self {
            rule: None,
            event: None,
            status: None,
            trigger_ref: None,
            rule_ref: None,
            trace_tag: None,
            include_total: true,
            limit: 0,
            offset: 0,
        }
    }
}

/// Result of [`EnforcementRepository::search`].
#[derive(Debug)]
pub struct EnforcementSearchResult {
    pub rows: Vec<Enforcement>,
    pub total: Option<u64>,
    pub has_next: bool,
}

#[derive(Debug, Clone)]
pub struct EnforcementCreateOrGetResult {
    pub enforcement: Enforcement,
    pub created: bool,
}

/// Repository for Event operations
pub struct EventRepository;

impl Repository for EventRepository {
    type Entity = Event;

    fn table_name() -> &'static str {
        "event"
    }
}

/// Input for creating a new event
#[derive(Debug, Clone)]
pub struct CreateEventInput {
    pub trigger: Option<Id>,
    pub trigger_ref: String,
    pub config: Option<JsonDict>,
    pub payload: Option<JsonDict>,
    pub trace_tag: Option<String>,
    pub source: Option<Id>,
    pub source_ref: Option<String>,
    pub rule: Option<Id>,
    pub rule_ref: Option<String>,
}

#[async_trait::async_trait]
impl FindById for EventRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let event = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, trigger, trigger_ref, config, payload, trace_tag, source, source_ref,
                   rule, rule_ref, created
            FROM event
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(event)
    }
}

#[async_trait::async_trait]
impl List for EventRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let events = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, trigger, trigger_ref, config, payload, trace_tag, source, source_ref,
                   rule, rule_ref, created
            FROM event
            ORDER BY created DESC
            LIMIT 1000
            "#,
        )
        .fetch_all(executor)
        .await?;

        Ok(events)
    }
}

#[async_trait::async_trait]
impl Create for EventRepository {
    type CreateInput = CreateEventInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let event = sqlx::query_as::<_, Event>(
            r#"
            INSERT INTO event (trigger, trigger_ref, config, payload, trace_tag, source, source_ref, rule, rule_ref)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id, trigger, trigger_ref, config, payload, trace_tag, source, source_ref,
                      rule, rule_ref, created
            "#,
        )
        .bind(input.trigger)
        .bind(&input.trigger_ref)
        .bind(&input.config)
        .bind(&input.payload)
        .bind(&input.trace_tag)
        .bind(input.source)
        .bind(&input.source_ref)
        .bind(input.rule)
        .bind(&input.rule_ref)
        .fetch_one(executor)
        .await?;

        Ok(event)
    }
}

#[async_trait::async_trait]
impl Delete for EventRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM event WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl EventRepository {
    /// Find events by trigger ID
    pub async fn find_by_trigger<'e, E>(executor: E, trigger_id: Id) -> Result<Vec<Event>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let events = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, trigger, trigger_ref, config, payload, trace_tag, source, source_ref,
                   rule, rule_ref, created
            FROM event
            WHERE trigger = $1
            ORDER BY created DESC
            LIMIT 1000
            "#,
        )
        .bind(trigger_id)
        .fetch_all(executor)
        .await?;

        Ok(events)
    }

    /// Find events by trigger ref
    pub async fn find_by_trigger_ref<'e, E>(executor: E, trigger_ref: &str) -> Result<Vec<Event>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let events = sqlx::query_as::<_, Event>(
            r#"
            SELECT id, trigger, trigger_ref, config, payload, trace_tag, source, source_ref,
                   rule, rule_ref, created
            FROM event
            WHERE trigger_ref = $1
            ORDER BY created DESC
            LIMIT 1000
            "#,
        )
        .bind(trigger_ref)
        .fetch_all(executor)
        .await?;

        Ok(events)
    }

    /// Search events with all filters pushed into SQL.
    ///
    /// Builds a dynamic query so that every filter, pagination, and the total
    /// count are handled in the database — no in-memory filtering or slicing.
    pub async fn search<'e, E>(db: E, filters: &EventSearchFilters) -> Result<EventSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let select_cols =
            "id, trigger, trigger_ref, config, payload, trace_tag, source, source_ref, rule, rule_ref, created";

        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {select_cols} FROM event"));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM event");

        let mut has_where = false;

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

        macro_rules! push_like_condition {
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
                qb.push(r" ESCAPE '\'");
                count_qb.push($cond_prefix);
                count_qb.push_bind($value);
                count_qb.push(r" ESCAPE '\'");
            }};
        }

        if let Some(trigger_id) = filters.trigger {
            push_condition!("trigger = ", trigger_id);
        }
        if let Some(ref trigger_ref) = filters.trigger_ref {
            if let Some(pattern) = ref_filter_like_pattern(trigger_ref) {
                push_like_condition!("trigger_ref LIKE ", pattern);
            } else {
                push_condition!("trigger_ref = ", trigger_ref.clone());
            }
        }
        if let Some(source_id) = filters.source {
            push_condition!("source = ", source_id);
        }
        if let Some(ref rule_ref) = filters.rule_ref {
            if let Some(pattern) = ref_filter_like_pattern(rule_ref) {
                push_like_condition!("rule_ref LIKE ", pattern);
            } else {
                push_condition!("rule_ref = ", rule_ref.clone());
            }
        }
        if let Some(trace_tag) = &filters.trace_tag {
            if !has_where {
                qb.push(" WHERE ");
                count_qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
                count_qb.push(" AND ");
            }
            let trace_exists = "(event.trace_tag = ";
            qb.push(trace_exists)
                .push_bind(trace_tag.clone())
                .push(
                    " OR EXISTS (\
                SELECT 1 FROM enforcement enf \
                JOIN execution e ON e.enforcement = enf.id \
                WHERE enf.event = event.id AND e.trace_tag = ",
                )
                .push_bind(trace_tag.clone())
                .push("))");
            count_qb
                .push(trace_exists)
                .push_bind(trace_tag.clone())
                .push(
                    " OR EXISTS (\
                SELECT 1 FROM enforcement enf \
                JOIN execution e ON e.enforcement = enf.id \
                WHERE enf.event = event.id AND e.trace_tag = ",
                )
                .push_bind(trace_tag.clone())
                .push("))");
        }

        // Suppress unused-assignment warning from the macro's last expansion.
        let _ = has_where;

        let total = if filters.include_total {
            let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;
            Some(total.max(0) as u64)
        } else {
            None
        };

        // Data query
        qb.push(" ORDER BY created DESC");
        qb.push(" LIMIT ");
        let query_limit = if filters.include_total {
            filters.limit
        } else {
            filters.limit.saturating_add(1)
        };
        qb.push_bind(query_limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let mut rows: Vec<Event> = qb.build_query_as().fetch_all(db).await?;
        let has_next = if let Some(total) = total {
            filters.offset as u64 + (rows.len() as u64) < total
        } else if rows.len() > filters.limit as usize {
            rows.truncate(filters.limit as usize);
            true
        } else {
            false
        };

        Ok(EventSearchResult {
            rows,
            total,
            has_next,
        })
    }

    /// Resolve one representative trace tag per event from linked executions.
    pub async fn trace_tags_by_event_ids<'e, E>(
        db: E,
        event_ids: &[Id],
    ) -> Result<HashMap<Id, String>>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        if event_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = sqlx::query_as::<_, (i64, String)>(
            r#"
            SELECT picked.event_id, picked.trace_tag
            FROM (
                SELECT DISTINCT ON (enf.event)
                    enf.event AS event_id,
                    e.trace_tag
                FROM enforcement enf
                JOIN execution e ON e.enforcement = enf.id
                WHERE enf.event = ANY($1)
                  AND e.trace_tag IS NOT NULL
                ORDER BY enf.event, e.created ASC, e.id ASC
            ) picked
            "#,
        )
        .bind(event_ids)
        .fetch_all(db)
        .await?;

        Ok(rows.into_iter().collect())
    }
}

// ============================================================================
// Enforcement Repository
// ============================================================================

/// Repository for Enforcement operations
pub struct EnforcementRepository;

impl Repository for EnforcementRepository {
    type Entity = Enforcement;

    fn table_name() -> &'static str {
        "enforcement"
    }
}

/// Input for creating a new enforcement
#[derive(Debug, Clone)]
pub struct CreateEnforcementInput {
    pub rule: Option<Id>,
    pub rule_ref: String,
    pub trigger_ref: String,
    pub config: Option<JsonDict>,
    pub event: Option<Id>,
    pub status: EnforcementStatus,
    pub payload: JsonDict,
    pub condition: EnforcementCondition,
    pub conditions: serde_json::Value,
}

/// Input for updating an enforcement
#[derive(Debug, Clone, Default)]
pub struct UpdateEnforcementInput {
    pub status: Option<EnforcementStatus>,
    pub payload: Option<JsonDict>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[async_trait::async_trait]
impl FindById for EnforcementRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let enforcement = sqlx::query_as::<_, Enforcement>(
            r#"
            SELECT id, rule, rule_ref, trigger_ref, config, event, status, payload,
                   condition, conditions, created, resolved_at
            FROM enforcement
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(enforcement)
    }
}

#[async_trait::async_trait]
impl List for EnforcementRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let enforcements = sqlx::query_as::<_, Enforcement>(
            r#"
            SELECT id, rule, rule_ref, trigger_ref, config, event, status, payload,
                   condition, conditions, created, resolved_at
            FROM enforcement
            ORDER BY created DESC
            LIMIT 1000
            "#,
        )
        .fetch_all(executor)
        .await?;

        Ok(enforcements)
    }
}

#[async_trait::async_trait]
impl Create for EnforcementRepository {
    type CreateInput = CreateEnforcementInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let enforcement = sqlx::query_as::<_, Enforcement>(
            r#"
            INSERT INTO enforcement (rule, rule_ref, trigger_ref, config, event, status,
                                     payload, condition, conditions)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id, rule, rule_ref, trigger_ref, config, event, status, payload,
                      condition, conditions, created, resolved_at
            "#,
        )
        .bind(input.rule)
        .bind(&input.rule_ref)
        .bind(&input.trigger_ref)
        .bind(&input.config)
        .bind(input.event)
        .bind(input.status)
        .bind(&input.payload)
        .bind(input.condition)
        .bind(&input.conditions)
        .fetch_one(executor)
        .await?;

        Ok(enforcement)
    }
}

#[async_trait::async_trait]
impl Update for EnforcementRepository {
    type UpdateInput = UpdateEnforcementInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.status.is_none() && input.payload.is_none() && input.resolved_at.is_none() {
            return Self::get_by_id(executor, id).await;
        }

        Self::update_with_locator(executor, input, |query| {
            query.push(" WHERE id = ");
            query.push_bind(id);
        })
        .await
    }
}

#[async_trait::async_trait]
impl Delete for EnforcementRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            r#"
            WITH deleted_enforcement AS (
                DELETE FROM enforcement
                WHERE id = $1
                RETURNING id
            ),
            deleted_enforcement_secrets AS (
                DELETE FROM execution_secret_value
                WHERE entity_type = 'enforcement_config'
                  AND entity_id IN (SELECT id FROM deleted_enforcement)
            )
            SELECT COUNT(*) AS deleted_count FROM deleted_enforcement
            "#,
        )
        .bind(id)
        .fetch_one(executor)
        .await?;
        let deleted_count: i64 = result.get("deleted_count");
        Ok(deleted_count > 0)
    }
}

impl EnforcementRepository {
    async fn update_with_locator<'e, E, F>(
        executor: E,
        input: UpdateEnforcementInput,
        where_clause: F,
    ) -> Result<Enforcement>
    where
        E: Executor<'e, Database = Postgres> + 'e,
        F: FnOnce(&mut QueryBuilder<'_, Postgres>),
    {
        let mut query = QueryBuilder::new("UPDATE enforcement SET ");
        let mut has_updates = false;

        if let Some(status) = input.status {
            query.push("status = ");
            query.push_bind(status);
            has_updates = true;
        }

        if let Some(payload) = &input.payload {
            if has_updates {
                query.push(", ");
            }
            query.push("payload = ");
            query.push_bind(payload);
            has_updates = true;
        }

        if let Some(resolved_at) = input.resolved_at {
            if has_updates {
                query.push(", ");
            }
            query.push("resolved_at = ");
            query.push_bind(resolved_at);
        }

        where_clause(&mut query);
        query.push(
            " RETURNING id, rule, rule_ref, trigger_ref, config, event, status, payload, \
             condition, conditions, created, resolved_at",
        );

        let enforcement = query
            .build_query_as::<Enforcement>()
            .fetch_one(executor)
            .await?;

        Ok(enforcement)
    }

    /// Update an enforcement using the loaded row's primary key.
    pub async fn update_loaded<'e, E>(
        executor: E,
        enforcement: &Enforcement,
        input: UpdateEnforcementInput,
    ) -> Result<Enforcement>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.status.is_none() && input.payload.is_none() && input.resolved_at.is_none() {
            return Ok(enforcement.clone());
        }

        Self::update_with_locator(executor, input, |query| {
            query.push(" WHERE id = ");
            query.push_bind(enforcement.id);
        })
        .await
    }

    pub async fn update_loaded_if_status<'e, E>(
        executor: E,
        enforcement: &Enforcement,
        expected_status: EnforcementStatus,
        input: UpdateEnforcementInput,
    ) -> Result<Option<Enforcement>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.status.is_none() && input.payload.is_none() && input.resolved_at.is_none() {
            return Ok(Some(enforcement.clone()));
        }

        let mut query = QueryBuilder::new("UPDATE enforcement SET ");
        let mut has_updates = false;

        if let Some(status) = input.status {
            query.push("status = ");
            query.push_bind(status);
            has_updates = true;
        }

        if let Some(payload) = &input.payload {
            if has_updates {
                query.push(", ");
            }
            query.push("payload = ");
            query.push_bind(payload);
            has_updates = true;
        }

        if let Some(resolved_at) = input.resolved_at {
            if has_updates {
                query.push(", ");
            }
            query.push("resolved_at = ");
            query.push_bind(resolved_at);
            has_updates = true;
        }

        if !has_updates {
            return Ok(Some(enforcement.clone()));
        }

        query.push(" WHERE id = ");
        query.push_bind(enforcement.id);
        query.push(" AND status = ");
        query.push_bind(expected_status);
        query.push(
            " RETURNING id, rule, rule_ref, trigger_ref, config, event, status, payload, \
             condition, conditions, created, resolved_at",
        );

        query
            .build_query_as::<Enforcement>()
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Find enforcements by rule ID
    pub async fn find_by_rule<'e, E>(executor: E, rule_id: Id) -> Result<Vec<Enforcement>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let enforcements = sqlx::query_as::<_, Enforcement>(
            r#"
            SELECT id, rule, rule_ref, trigger_ref, config, event, status, payload,
                   condition, conditions, created, resolved_at
            FROM enforcement
            WHERE rule = $1
            ORDER BY created DESC
            "#,
        )
        .bind(rule_id)
        .fetch_all(executor)
        .await?;

        Ok(enforcements)
    }

    /// Find enforcements by status
    pub async fn find_by_status<'e, E>(
        executor: E,
        status: EnforcementStatus,
    ) -> Result<Vec<Enforcement>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let enforcements = sqlx::query_as::<_, Enforcement>(
            r#"
            SELECT id, rule, rule_ref, trigger_ref, config, event, status, payload,
                   condition, conditions, created, resolved_at
            FROM enforcement
            WHERE status = $1
            ORDER BY created DESC
            "#,
        )
        .bind(status)
        .fetch_all(executor)
        .await?;

        Ok(enforcements)
    }

    /// Find enforcements by event ID
    pub async fn find_by_event<'e, E>(executor: E, event_id: Id) -> Result<Vec<Enforcement>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let enforcements = sqlx::query_as::<_, Enforcement>(
            r#"
            SELECT id, rule, rule_ref, trigger_ref, config, event, status, payload,
                   condition, conditions, created, resolved_at
            FROM enforcement
            WHERE event = $1
            ORDER BY created DESC
            "#,
        )
        .bind(event_id)
        .fetch_all(executor)
        .await?;

        Ok(enforcements)
    }

    pub async fn find_by_rule_and_event<'e, E>(
        executor: E,
        rule_id: Id,
        event_id: Id,
    ) -> Result<Option<Enforcement>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Enforcement>(
            r#"
            SELECT id, rule, rule_ref, trigger_ref, config, event, status, payload,
                   condition, conditions, created, resolved_at
            FROM enforcement
            WHERE rule = $1 AND event = $2
            LIMIT 1
            "#,
        )
        .bind(rule_id)
        .bind(event_id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn create_or_get_by_rule_event<'e, E>(
        executor: E,
        input: CreateEnforcementInput,
    ) -> Result<EnforcementCreateOrGetResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let (Some(rule_id), Some(event_id)) = (input.rule, input.event) else {
            let enforcement = Self::create(executor, input).await?;
            return Ok(EnforcementCreateOrGetResult {
                enforcement,
                created: true,
            });
        };

        let inserted = sqlx::query_as::<_, Enforcement>(
            r#"
            INSERT INTO enforcement (rule, rule_ref, trigger_ref, config, event, status,
                                     payload, condition, conditions)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (rule, event) WHERE rule IS NOT NULL AND event IS NOT NULL DO NOTHING
            RETURNING id, rule, rule_ref, trigger_ref, config, event, status, payload,
                      condition, conditions, created, resolved_at
            "#,
        )
        .bind(input.rule)
        .bind(&input.rule_ref)
        .bind(&input.trigger_ref)
        .bind(&input.config)
        .bind(input.event)
        .bind(input.status)
        .bind(&input.payload)
        .bind(input.condition)
        .bind(&input.conditions)
        .fetch_optional(executor)
        .await?;

        if let Some(enforcement) = inserted {
            return Ok(EnforcementCreateOrGetResult {
                enforcement,
                created: true,
            });
        }

        let enforcement = Self::find_by_rule_and_event(executor, rule_id, event_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "enforcement for rule {} and event {} disappeared after dedupe conflict",
                    rule_id,
                    event_id
                )
            })?;

        Ok(EnforcementCreateOrGetResult {
            enforcement,
            created: false,
        })
    }

    /// Search enforcements with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    pub async fn search<'e, E>(
        db: E,
        filters: &EnforcementSearchFilters,
    ) -> Result<EnforcementSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let select_cols = "id, rule, rule_ref, trigger_ref, config, event, status, payload, condition, conditions, created, resolved_at";

        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {select_cols} FROM enforcement"));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM enforcement");

        let mut has_where = false;

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

        if let Some(status) = &filters.status {
            push_condition!("status = ", *status);
        }
        if let Some(rule_id) = filters.rule {
            push_condition!("rule = ", rule_id);
        }
        if let Some(event_id) = filters.event {
            push_condition!("event = ", event_id);
        }
        if let Some(ref trigger_ref) = filters.trigger_ref {
            push_condition!("trigger_ref = ", trigger_ref.clone());
        }
        if let Some(ref rule_ref) = filters.rule_ref {
            push_condition!("rule_ref = ", rule_ref.clone());
        }
        if let Some(trace_tag) = &filters.trace_tag {
            if !has_where {
                qb.push(" WHERE ");
                count_qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
                count_qb.push(" AND ");
            }
            let trace_exists = "EXISTS (SELECT 1 FROM execution e WHERE e.enforcement = enforcement.id AND e.trace_tag = ";
            qb.push(trace_exists).push_bind(trace_tag.clone()).push(")");
            count_qb
                .push(trace_exists)
                .push_bind(trace_tag.clone())
                .push(")");
        }

        // Suppress unused-assignment warning from the macro's last expansion.
        let _ = has_where;

        let total = if filters.include_total {
            let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;
            Some(total.max(0) as u64)
        } else {
            None
        };

        // Data query
        qb.push(" ORDER BY created DESC");
        qb.push(" LIMIT ");
        let query_limit = if filters.include_total {
            filters.limit
        } else {
            filters.limit.saturating_add(1)
        };
        qb.push_bind(query_limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let mut rows: Vec<Enforcement> = qb.build_query_as().fetch_all(db).await?;
        let has_next = if let Some(total) = total {
            filters.offset as u64 + (rows.len() as u64) < total
        } else if rows.len() > filters.limit as usize {
            rows.truncate(filters.limit as usize);
            true
        } else {
            false
        };

        Ok(EnforcementSearchResult {
            rows,
            total,
            has_next,
        })
    }

    /// Resolve one representative trace tag per enforcement from linked executions.
    pub async fn trace_tags_by_enforcement_ids<'e, E>(
        db: E,
        enforcement_ids: &[Id],
    ) -> Result<HashMap<Id, String>>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        if enforcement_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = sqlx::query_as::<_, (i64, String)>(
            r#"
            SELECT picked.enforcement_id, picked.trace_tag
            FROM (
                SELECT DISTINCT ON (e.enforcement)
                    e.enforcement AS enforcement_id,
                    e.trace_tag
                FROM execution e
                WHERE e.enforcement = ANY($1)
                  AND e.trace_tag IS NOT NULL
                ORDER BY e.enforcement, e.created ASC, e.id ASC
            ) picked
            "#,
        )
        .bind(enforcement_ids)
        .fetch_all(db)
        .await?;

        Ok(rows.into_iter().collect())
    }
}
