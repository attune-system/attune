//! Work queue repositories for first-class business queues.

use chrono::{DateTime, Utc};
use sqlx::{Executor, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::models::{
    work_queue::{
        WorkQueue, WorkQueueBatchCoalescingConfig, WorkQueueConfig, WorkQueueDispatch,
        WorkQueueItem, WORK_QUEUE_DISPATCH_SELECT_COLUMNS, WORK_QUEUE_ITEM_SELECT_COLUMNS,
        WORK_QUEUE_SELECT_COLUMNS,
    },
    ActionReferenceVisibility, Id, JsonDict, WorkQueueBatchMode, WorkQueueDispatchStatus,
    WorkQueueItemStatus, WorkQueueUpdateStrategy,
};
use crate::queue_definition::{
    validate_queue_reference_visibility_config, validate_work_queue_batch_settings,
    validate_work_queue_config, validate_work_queue_config_for_batch_mode,
    validate_work_queue_item_schema,
};
use crate::schema::RefValidator;
use crate::trace_tag::default_queue_item_trace_tag;
use crate::{Error, Result};

use super::{Create, Delete, FindById, FindByRef, List, Patch, Repository, Update};

#[cfg(test)]
mod tests {
    use crate::schema::RefValidator;

    #[test]
    fn validate_queue_ref_accepts_refs_matching_db_constraint() {
        for queue_ref in ["queue", "queue_1", "queue-name", "queue.segment_2", "q.1"] {
            assert!(
                RefValidator::validate_work_queue_ref(queue_ref).is_ok(),
                "{queue_ref} should be valid"
            );
        }
    }

    #[test]
    fn validate_queue_ref_rejects_refs_failing_db_constraint() {
        for queue_ref in [
            "",
            "1queue",
            "-queue",
            "_queue",
            ".queue",
            "queue.",
            "queue..part",
        ] {
            assert!(
                RefValidator::validate_work_queue_ref(queue_ref).is_err(),
                "{queue_ref} should be invalid"
            );
        }
    }
}

// ============================================================================
// WorkQueueRepository
// ============================================================================

#[derive(Debug, Clone, Default)]
pub struct WorkQueueSearchFilters {
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub dispatch_action: Option<Id>,
    pub enabled: Option<bool>,
    pub is_adhoc: Option<bool>,
    pub search: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug)]
pub struct WorkQueueSearchResult {
    pub rows: Vec<WorkQueue>,
    pub total: u64,
}

pub struct WorkQueueRepository;

impl Repository for WorkQueueRepository {
    type Entity = WorkQueue;

    fn table_name() -> &'static str {
        "work_queue"
    }
}

#[derive(Debug, Clone)]
pub struct CreateWorkQueueInput {
    pub r#ref: String,
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub is_adhoc: bool,
    pub label: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub accepting_new_items: bool,
    pub dispatch_action: Option<Id>,
    pub dispatch_action_ref: String,
    pub default_priority: i32,
    pub allow_pending_update: bool,
    pub update_strategy: WorkQueueUpdateStrategy,
    pub batch_mode: WorkQueueBatchMode,
    pub item_schema: JsonDict,
    pub action_params: JsonDict,
    pub trace_tag_template: Option<String>,
    pub permission_set_refs: Option<Vec<String>>,
    pub config: JsonDict,
    pub reference_visibility: ActionReferenceVisibility,
    pub reference_allowed_pack_refs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateWorkQueueInput {
    pub pack: Option<Patch<Id>>,
    pub pack_ref: Option<Patch<String>>,
    pub is_adhoc: Option<bool>,
    pub label: Option<String>,
    pub description: Option<Patch<String>>,
    pub enabled: Option<bool>,
    pub accepting_new_items: Option<bool>,
    pub dispatch_action: Option<Patch<Id>>,
    pub dispatch_action_ref: Option<String>,
    pub default_priority: Option<i32>,
    pub allow_pending_update: Option<bool>,
    pub update_strategy: Option<WorkQueueUpdateStrategy>,
    pub batch_mode: Option<WorkQueueBatchMode>,
    pub item_schema: Option<JsonDict>,
    pub action_params: Option<JsonDict>,
    pub trace_tag_template: Option<Patch<String>>,
    pub permission_set_refs: Option<Patch<Vec<String>>>,
    pub config: Option<JsonDict>,
    pub reference_visibility: Option<ActionReferenceVisibility>,
    pub reference_allowed_pack_refs: Option<Vec<String>>,
}

#[async_trait::async_trait]
impl FindById for WorkQueueRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue WHERE id = $1",
            WORK_QUEUE_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueue>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl FindByRef for WorkQueueRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue WHERE ref = $1",
            WORK_QUEUE_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueue>(&query)
            .bind(ref_str)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for WorkQueueRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue ORDER BY ref ASC LIMIT 1000",
            WORK_QUEUE_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueue>(&query)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for WorkQueueRepository {
    type CreateInput = CreateWorkQueueInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        RefValidator::validate_work_queue_ref(&input.r#ref)?;
        RefValidator::validate_component_ref(&input.dispatch_action_ref)?;
        validate_work_queue_item_schema(&input.item_schema)?;
        crate::queue_definition::validate_work_queue_action_params(&input.action_params)?;
        validate_work_queue_config_for_batch_mode(input.batch_mode, &input.config)?;
        validate_queue_reference_visibility_config(
            input.reference_visibility,
            &input.reference_allowed_pack_refs,
        )?;

        let query = format!(
            "INSERT INTO work_queue \
             (ref, pack, pack_ref, is_adhoc, label, description, enabled, accepting_new_items, \
                 dispatch_action, dispatch_action_ref, default_priority, allow_pending_update, update_strategy, \
                 batch_mode, item_schema, action_params, trace_tag_template, permission_set_refs, config, \
                 reference_visibility, reference_allowed_pack_refs) \
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21) \
             RETURNING {}",
            WORK_QUEUE_SELECT_COLUMNS
        );

        sqlx::query_as::<_, WorkQueue>(&query)
            .bind(&input.r#ref)
            .bind(input.pack)
            .bind(&input.pack_ref)
            .bind(input.is_adhoc)
            .bind(&input.label)
            .bind(&input.description)
            .bind(input.enabled)
            .bind(input.accepting_new_items)
            .bind(input.dispatch_action)
            .bind(&input.dispatch_action_ref)
            .bind(input.default_priority)
            .bind(input.allow_pending_update)
            .bind(input.update_strategy)
            .bind(input.batch_mode)
            .bind(&input.item_schema)
            .bind(&input.action_params)
            .bind(&input.trace_tag_template)
            .bind(&input.permission_set_refs)
            .bind(&input.config)
            .bind(input.reference_visibility)
            .bind(&input.reference_allowed_pack_refs)
            .fetch_one(executor)
            .await
            .map_err(|e| {
                if let sqlx::Error::Database(ref db_err) = e {
                    if db_err.is_unique_violation() {
                        return Error::already_exists("WorkQueue", "ref", &input.r#ref);
                    }
                }
                e.into()
            })
    }
}

#[async_trait::async_trait]
impl Update for WorkQueueRepository {
    type UpdateInput = UpdateWorkQueueInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if let Some(dispatch_action_ref) = &input.dispatch_action_ref {
            RefValidator::validate_component_ref(dispatch_action_ref)?;
        }
        if let Some(item_schema) = &input.item_schema {
            validate_work_queue_item_schema(item_schema)?;
        }
        if let Some(action_params) = &input.action_params {
            crate::queue_definition::validate_work_queue_action_params(action_params)?;
        }
        if let Some(config) = &input.config {
            validate_work_queue_config(config)?;
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
                RefValidator::validate_pack_ref(pack_ref)?;
            }
        }
        if let (Some(batch_mode), Some(config)) = (input.batch_mode, input.config.as_ref()) {
            let parsed_config = validate_work_queue_config(config)?;
            validate_work_queue_batch_settings(batch_mode, &parsed_config)?;
        }

        let mut query = QueryBuilder::new("UPDATE work_queue SET ");
        let mut has_updates = false;

        if let Some(pack) = &input.pack {
            query.push("pack = ");
            match pack {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<Id>::None),
            };
            has_updates = true;
        }

        if let Some(pack_ref) = &input.pack_ref {
            if has_updates {
                query.push(", ");
            }
            query.push("pack_ref = ");
            match pack_ref {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }

        if let Some(is_adhoc) = input.is_adhoc {
            if has_updates {
                query.push(", ");
            }
            query.push("is_adhoc = ").push_bind(is_adhoc);
            has_updates = true;
        }

        if let Some(label) = &input.label {
            if has_updates {
                query.push(", ");
            }
            query.push("label = ").push_bind(label);
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

        if let Some(enabled) = input.enabled {
            if has_updates {
                query.push(", ");
            }
            query.push("enabled = ").push_bind(enabled);
            has_updates = true;
        }

        if let Some(accepting_new_items) = input.accepting_new_items {
            if has_updates {
                query.push(", ");
            }
            query
                .push("accepting_new_items = ")
                .push_bind(accepting_new_items);
            has_updates = true;
        }

        if let Some(dispatch_action) = &input.dispatch_action {
            if has_updates {
                query.push(", ");
            }
            query.push("dispatch_action = ");
            match dispatch_action {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<Id>::None),
            };
            has_updates = true;
        }

        if let Some(dispatch_action_ref) = &input.dispatch_action_ref {
            if has_updates {
                query.push(", ");
            }
            query
                .push("dispatch_action_ref = ")
                .push_bind(dispatch_action_ref);
            has_updates = true;
        }

        if let Some(default_priority) = input.default_priority {
            if has_updates {
                query.push(", ");
            }
            query
                .push("default_priority = ")
                .push_bind(default_priority);
            has_updates = true;
        }

        if let Some(allow_pending_update) = input.allow_pending_update {
            if has_updates {
                query.push(", ");
            }
            query
                .push("allow_pending_update = ")
                .push_bind(allow_pending_update);
            has_updates = true;
        }

        if let Some(update_strategy) = input.update_strategy {
            if has_updates {
                query.push(", ");
            }
            query.push("update_strategy = ").push_bind(update_strategy);
            has_updates = true;
        }

        if let Some(batch_mode) = input.batch_mode {
            if has_updates {
                query.push(", ");
            }
            query.push("batch_mode = ").push_bind(batch_mode);
            has_updates = true;
        }

        if let Some(item_schema) = &input.item_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("item_schema = ").push_bind(item_schema);
            has_updates = true;
        }

        if let Some(action_params) = &input.action_params {
            if has_updates {
                query.push(", ");
            }
            query.push("action_params = ").push_bind(action_params);
            has_updates = true;
        }

        if let Some(trace_tag_template) = &input.trace_tag_template {
            if has_updates {
                query.push(", ");
            }
            query.push("trace_tag_template = ");
            match trace_tag_template {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }

        if let Some(permission_set_refs) = &input.permission_set_refs {
            if has_updates {
                query.push(", ");
            }
            query.push("permission_set_refs = ");
            match permission_set_refs {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<Vec<String>>::None),
            };
            has_updates = true;
        }

        if let Some(config) = &input.config {
            if has_updates {
                query.push(", ");
            }
            query.push("config = ").push_bind(config);
            has_updates = true;
        }

        if let Some(reference_visibility) = input.reference_visibility {
            if has_updates {
                query.push(", ");
            }
            query
                .push("reference_visibility = ")
                .push_bind(reference_visibility);
            has_updates = true;
        }

        if let Some(reference_allowed_pack_refs) = &input.reference_allowed_pack_refs {
            if has_updates {
                query.push(", ");
            }
            query
                .push("reference_allowed_pack_refs = ")
                .push_bind(reference_allowed_pack_refs);
            has_updates = true;
        }

        if !has_updates {
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(" RETURNING ").push(WORK_QUEUE_SELECT_COLUMNS);

        query
            .build_query_as::<WorkQueue>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for WorkQueueRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM work_queue WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl WorkQueueRepository {
    pub async fn find_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Vec<WorkQueue>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue WHERE pack = $1 ORDER BY ref ASC",
            WORK_QUEUE_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueue>(&query)
            .bind(pack_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn list_enabled<'e, E>(executor: E) -> Result<Vec<WorkQueue>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue WHERE enabled = TRUE ORDER BY ref ASC",
            WORK_QUEUE_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueue>(&query)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn list_by_pack_ref<'e, E>(executor: E, pack_ref: &str) -> Result<Vec<WorkQueue>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue WHERE pack_ref = $1 ORDER BY ref ASC",
            WORK_QUEUE_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueue>(&query)
            .bind(pack_ref)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn search<'e, E>(
        db: E,
        filters: &WorkQueueSearchFilters,
    ) -> Result<WorkQueueSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let mut qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(format!(
            "SELECT {} FROM work_queue",
            WORK_QUEUE_SELECT_COLUMNS
        ));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM work_queue");
        let mut has_where = false;

        macro_rules! push_condition {
            ($sql:expr, $value:expr) => {{
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push($sql).push_bind($value.clone());
                count_qb.push($sql).push_bind($value);
            }};
        }

        if let Some(pack) = filters.pack {
            push_condition!("pack = ", pack);
        }
        if let Some(ref pack_ref) = filters.pack_ref {
            push_condition!("pack_ref = ", pack_ref.clone());
        }
        if let Some(dispatch_action) = filters.dispatch_action {
            push_condition!("dispatch_action = ", dispatch_action);
        }
        if let Some(enabled) = filters.enabled {
            push_condition!("enabled = ", enabled);
        }
        if let Some(is_adhoc) = filters.is_adhoc {
            push_condition!("is_adhoc = ", is_adhoc);
        }
        if let Some(ref search) = filters.search {
            let search = format!("%{}%", search.trim());
            if !has_where {
                qb.push(" WHERE ");
                count_qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
                count_qb.push(" AND ");
            }
            qb.push("(ref ILIKE ")
                .push_bind(search.clone())
                .push(" OR label ILIKE ")
                .push_bind(search.clone())
                .push(" OR COALESCE(description, '') ILIKE ")
                .push_bind(search.clone())
                .push(")");
            count_qb
                .push("(ref ILIKE ")
                .push_bind(search.clone())
                .push(" OR label ILIKE ")
                .push_bind(search.clone())
                .push(" OR COALESCE(description, '') ILIKE ")
                .push_bind(search)
                .push(")");
        }

        let _ = has_where;

        let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;

        qb.push(" ORDER BY ref ASC LIMIT ")
            .push_bind(filters.limit as i64)
            .push(" OFFSET ")
            .push_bind(filters.offset as i64);

        let rows = qb.build_query_as::<WorkQueue>().fetch_all(db).await?;

        Ok(WorkQueueSearchResult {
            rows,
            total: total.max(0) as u64,
        })
    }

    pub fn parse_config(queue: &WorkQueue) -> Result<WorkQueueConfig> {
        serde_json::from_value(queue.config.clone()).map_err(Into::into)
    }

    pub async fn delete_non_adhoc_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        keep_refs: &[String],
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = if keep_refs.is_empty() {
            sqlx::query("DELETE FROM work_queue WHERE pack = $1 AND is_adhoc = false")
                .bind(pack_id)
                .execute(executor)
                .await?
        } else {
            sqlx::query(
                "DELETE FROM work_queue WHERE pack = $1 AND is_adhoc = false AND ref != ALL($2)",
            )
            .bind(pack_id)
            .bind(keep_refs)
            .execute(executor)
            .await?
        };

        Ok(result.rows_affected())
    }
}

// ============================================================================
// WorkQueueItemRepository
// ============================================================================

#[derive(Debug, Clone, Default)]
pub struct WorkQueueItemSearchFilters {
    pub queue: Option<Id>,
    pub queue_ref: Option<String>,
    pub item_key: Option<String>,
    pub statuses: Option<Vec<WorkQueueItemStatus>>,
    pub enqueue_source: Option<String>,
    pub requested_by_identity: Option<Id>,
    pub requested_by_execution: Option<Id>,
    pub requested_by_enforcement: Option<Id>,
    pub leased_execution: Option<Id>,
    pub lease_token: Option<Uuid>,
    pub only_expired_leases: bool,
    pub now: Option<DateTime<Utc>>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug)]
pub struct WorkQueueItemSearchResult {
    pub rows: Vec<WorkQueueItem>,
    pub total: u64,
}

#[derive(Debug, Clone)]
pub struct WorkQueueItemJsonPathSelector {
    pub path: String,
    pub vars: JsonDict,
}

#[derive(Debug, Clone)]
pub struct CreateWorkQueueItemInput {
    pub queue: Id,
    pub queue_ref: String,
    pub item_key: Option<String>,
    pub priority: i32,
    pub status: WorkQueueItemStatus,
    pub payload: JsonDict,
    pub metadata: JsonDict,
    pub trace_tag: Option<String>,
    pub enqueue_source: String,
    pub requested_by_identity: Option<Id>,
    pub requested_by_execution: Option<Id>,
    pub requested_by_enforcement: Option<Id>,
    pub leased_execution: Option<Id>,
    pub lease_token: Option<Uuid>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub attempt_count: i32,
    pub last_error: Option<JsonDict>,
    pub ack_summary: Option<JsonDict>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateWorkQueueItemInput {
    pub item_key: Option<Patch<String>>,
    pub priority: Option<i32>,
    pub status: Option<WorkQueueItemStatus>,
    pub payload: Option<JsonDict>,
    pub metadata: Option<JsonDict>,
    pub trace_tag: Option<Patch<String>>,
    pub enqueue_source: Option<String>,
    pub requested_by_identity: Option<Patch<Id>>,
    pub requested_by_execution: Option<Patch<Id>>,
    pub requested_by_enforcement: Option<Patch<Id>>,
    pub leased_execution: Option<Patch<Id>>,
    pub lease_token: Option<Patch<Uuid>>,
    pub lease_expires_at: Option<Patch<DateTime<Utc>>>,
    pub attempt_count: Option<i32>,
    pub last_error: Option<Patch<JsonDict>>,
    pub ack_summary: Option<Patch<JsonDict>>,
}

#[derive(Debug, Clone)]
pub struct LeaseWorkQueueItemsInput {
    pub queue: Id,
    pub ready_statuses: Vec<WorkQueueItemStatus>,
    pub limit: i64,
    pub batch_coalescing: Option<WorkQueueBatchCoalescingConfig>,
    pub leased_execution: Option<Id>,
    pub lease_token: Uuid,
    pub lease_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ReleaseWorkQueueLeaseInput {
    pub lease_token: Uuid,
    pub new_status: WorkQueueItemStatus,
    pub leased_execution: Option<Id>,
    pub last_error: Option<JsonDict>,
    pub ack_summary: Option<JsonDict>,
}

pub struct WorkQueueItemRepository;

impl Repository for WorkQueueItemRepository {
    type Entity = WorkQueueItem;

    fn table_name() -> &'static str {
        "work_queue_item"
    }
}

#[async_trait::async_trait]
impl FindById for WorkQueueItemRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_item WHERE id = $1",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for WorkQueueItemRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_item ORDER BY created DESC LIMIT 1000",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueItem>(&query)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for WorkQueueItemRepository {
    type CreateInput = CreateWorkQueueItemInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut input = input;
        if input.trace_tag.is_none() {
            input.trace_tag = Some(default_queue_item_trace_tag(
                &input.queue_ref,
                Utc::now().timestamp_millis(),
            )?);
        }
        let query = format!(
            "INSERT INTO work_queue_item \
             (queue, queue_ref, item_key, priority, status, payload, metadata, trace_tag, enqueue_source, \
              requested_by_identity, requested_by_execution, requested_by_enforcement, \
              leased_execution, lease_token, lease_expires_at, attempt_count, last_error, \
              ack_summary) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) \
             RETURNING {}",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );

        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(input.queue)
            .bind(&input.queue_ref)
            .bind(&input.item_key)
            .bind(input.priority)
            .bind(input.status)
            .bind(&input.payload)
            .bind(&input.metadata)
            .bind(&input.trace_tag)
            .bind(&input.enqueue_source)
            .bind(input.requested_by_identity)
            .bind(input.requested_by_execution)
            .bind(input.requested_by_enforcement)
            .bind(input.leased_execution)
            .bind(input.lease_token)
            .bind(input.lease_expires_at)
            .bind(input.attempt_count)
            .bind(&input.last_error)
            .bind(&input.ack_summary)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for WorkQueueItemRepository {
    type UpdateInput = UpdateWorkQueueItemInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        Self::update_if_statuses(executor, id, &[], input)
            .await?
            .ok_or_else(|| Error::not_found("WorkQueueItem", "id", id.to_string()))
    }
}

#[async_trait::async_trait]
impl Delete for WorkQueueItemRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM work_queue_item WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl WorkQueueItemRepository {
    pub const MUTABLE_PENDING_STATUSES: [WorkQueueItemStatus; 2] =
        [WorkQueueItemStatus::Queued, WorkQueueItemStatus::Retry];

    pub fn mutable_pending_statuses() -> &'static [WorkQueueItemStatus] {
        &Self::MUTABLE_PENDING_STATUSES
    }

    pub fn is_mutable_pending_status(status: WorkQueueItemStatus) -> bool {
        Self::MUTABLE_PENDING_STATUSES.contains(&status)
    }

    pub async fn list_by_related_executions<'e, E>(
        executor: E,
        execution_ids: &[Id],
    ) -> Result<Vec<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if execution_ids.is_empty() {
            return Ok(Vec::new());
        }

        let query = format!(
            "SELECT {} FROM work_queue_item \
             WHERE requested_by_execution = ANY($1) OR leased_execution = ANY($1) \
             ORDER BY created ASC, id ASC",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(execution_ids)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    fn selected_item_document_sql() -> &'static str {
        "jsonb_strip_nulls(jsonb_build_object(\
            'payload', payload, \
            'metadata', metadata, \
            'item_key', item_key, \
            'priority', priority, \
            'status', status, \
            'enqueue_source', enqueue_source, \
            'attempt_count', attempt_count\
        ))"
    }

    pub async fn count_selected_mutable<'e, E>(
        executor: E,
        queue: Id,
        selector: &WorkQueueItemJsonPathSelector,
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT COUNT(*) FROM work_queue_item \
             WHERE queue = $1 \
               AND status = ANY($2::work_queue_item_status_enum[]) \
               AND jsonb_path_exists({}, $3::jsonpath, $4::jsonb)",
            Self::selected_item_document_sql()
        );
        let total: i64 = sqlx::query_scalar(&query)
            .bind(queue)
            .bind(Self::mutable_pending_statuses())
            .bind(&selector.path)
            .bind(&selector.vars)
            .fetch_one(executor)
            .await?;

        Ok(total.max(0) as u64)
    }

    pub async fn preview_selected_mutable<'e, E>(
        db: E,
        queue: Id,
        selector: &WorkQueueItemJsonPathSelector,
        limit: u32,
    ) -> Result<WorkQueueItemSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let total = Self::count_selected_mutable(db, queue, selector).await?;
        let query = format!(
            "SELECT {} FROM work_queue_item \
             WHERE queue = $1 \
               AND status = ANY($2::work_queue_item_status_enum[]) \
               AND jsonb_path_exists({}, $3::jsonpath, $4::jsonb) \
             ORDER BY priority DESC, created ASC, id ASC \
             LIMIT $5",
            WORK_QUEUE_ITEM_SELECT_COLUMNS,
            Self::selected_item_document_sql()
        );
        let rows = sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(queue)
            .bind(Self::mutable_pending_statuses())
            .bind(&selector.path)
            .bind(&selector.vars)
            .bind(limit as i64)
            .fetch_all(db)
            .await?;

        Ok(WorkQueueItemSearchResult { rows, total })
    }

    pub async fn list_selected_mutable<'e, E>(
        executor: E,
        queue: Id,
        selector: &WorkQueueItemJsonPathSelector,
    ) -> Result<Vec<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_item \
             WHERE queue = $1 \
               AND status = ANY($2::work_queue_item_status_enum[]) \
               AND jsonb_path_exists({}, $3::jsonpath, $4::jsonb) \
             ORDER BY priority DESC, created ASC, id ASC",
            WORK_QUEUE_ITEM_SELECT_COLUMNS,
            Self::selected_item_document_sql()
        );
        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(queue)
            .bind(Self::mutable_pending_statuses())
            .bind(&selector.path)
            .bind(&selector.vars)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn cancel_selected_mutable<'e, E>(
        executor: E,
        queue: Id,
        selector: &WorkQueueItemJsonPathSelector,
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "UPDATE work_queue_item \
             SET status = 'cancelled'::work_queue_item_status_enum, updated = NOW() \
             WHERE queue = $1 \
               AND status = ANY($2::work_queue_item_status_enum[]) \
               AND jsonb_path_exists({}, $3::jsonpath, $4::jsonb)",
            Self::selected_item_document_sql()
        );
        let result = sqlx::query(&query)
            .bind(queue)
            .bind(Self::mutable_pending_statuses())
            .bind(&selector.path)
            .bind(&selector.vars)
            .execute(executor)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn reprioritize_selected_mutable<'e, E>(
        executor: E,
        queue: Id,
        selector: &WorkQueueItemJsonPathSelector,
        priority: i32,
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "UPDATE work_queue_item \
             SET priority = $5, updated = NOW() \
             WHERE queue = $1 \
               AND status = ANY($2::work_queue_item_status_enum[]) \
               AND jsonb_path_exists({}, $3::jsonpath, $4::jsonb)",
            Self::selected_item_document_sql()
        );
        let result = sqlx::query(&query)
            .bind(queue)
            .bind(Self::mutable_pending_statuses())
            .bind(&selector.path)
            .bind(&selector.vars)
            .bind(priority)
            .execute(executor)
            .await?;
        Ok(result.rows_affected())
    }

    async fn update_internal<'e, E>(
        executor: E,
        id: i64,
        allowed_statuses: Option<&[WorkQueueItemStatus]>,
        input: UpdateWorkQueueItemInput,
    ) -> Result<Option<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query = QueryBuilder::new("UPDATE work_queue_item SET ");
        let mut has_updates = false;

        if let Some(item_key) = &input.item_key {
            query.push("item_key = ");
            match item_key {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }

        macro_rules! push_simple_field {
            ($field:expr, $column:expr) => {
                if let Some(value) = $field {
                    if has_updates {
                        query.push(", ");
                    }
                    query.push($column).push(" = ").push_bind(value);
                    has_updates = true;
                }
            };
        }

        push_simple_field!(input.priority, "priority");
        push_simple_field!(input.status, "status");
        if let Some(payload) = &input.payload {
            if has_updates {
                query.push(", ");
            }
            query.push("payload = ").push_bind(payload);
            has_updates = true;
        }
        if let Some(metadata) = &input.metadata {
            if has_updates {
                query.push(", ");
            }
            query.push("metadata = ").push_bind(metadata);
            has_updates = true;
        }
        if let Some(trace_tag) = &input.trace_tag {
            if has_updates {
                query.push(", ");
            }
            query.push("trace_tag = ");
            match trace_tag {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }
        if let Some(enqueue_source) = &input.enqueue_source {
            if has_updates {
                query.push(", ");
            }
            query.push("enqueue_source = ").push_bind(enqueue_source);
            has_updates = true;
        }

        macro_rules! push_patch_field {
            ($field:expr, $column:expr, $clear_ty:ty) => {
                if let Some(value) = &$field {
                    if has_updates {
                        query.push(", ");
                    }
                    query.push($column).push(" = ");
                    match value {
                        Patch::Set(inner) => query.push_bind(inner),
                        Patch::Clear => query.push_bind(Option::<$clear_ty>::None),
                    };
                    has_updates = true;
                }
            };
        }

        push_patch_field!(input.requested_by_identity, "requested_by_identity", Id);
        push_patch_field!(input.requested_by_execution, "requested_by_execution", Id);
        push_patch_field!(
            input.requested_by_enforcement,
            "requested_by_enforcement",
            Id
        );
        push_patch_field!(input.leased_execution, "leased_execution", Id);
        push_patch_field!(input.lease_token, "lease_token", Uuid);
        push_patch_field!(input.lease_expires_at, "lease_expires_at", DateTime<Utc>);
        push_simple_field!(input.attempt_count, "attempt_count");
        push_patch_field!(input.last_error, "last_error", JsonDict);
        push_patch_field!(input.ack_summary, "ack_summary", JsonDict);

        if !has_updates {
            let existing = Self::find_by_id(executor, id).await?;
            return Ok(existing.filter(|item| {
                allowed_statuses.is_none_or(|statuses| statuses.contains(&item.status))
            }));
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        if let Some(statuses) = allowed_statuses {
            if !statuses.is_empty() {
                query
                    .push(" AND status = ANY(")
                    .push_bind(statuses.to_vec())
                    .push("::work_queue_item_status_enum[])");
            }
        }
        query
            .push(" RETURNING ")
            .push(WORK_QUEUE_ITEM_SELECT_COLUMNS);

        query
            .build_query_as::<WorkQueueItem>()
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn enqueue<'e, E>(
        executor: E,
        input: CreateWorkQueueItemInput,
    ) -> Result<WorkQueueItem>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        Self::create(executor, input).await
    }

    pub async fn search<'e, E>(
        db: E,
        filters: &WorkQueueItemSearchFilters,
    ) -> Result<WorkQueueItemSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let mut qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(format!(
            "SELECT {} FROM work_queue_item",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        ));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM work_queue_item");
        let mut has_where = false;

        macro_rules! push_condition {
            ($sql:expr, $value:expr) => {{
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push($sql).push_bind($value.clone());
                count_qb.push($sql).push_bind($value);
            }};
        }

        if let Some(queue) = filters.queue {
            push_condition!("queue = ", queue);
        }
        if let Some(ref queue_ref) = filters.queue_ref {
            push_condition!("queue_ref = ", queue_ref.clone());
        }
        if let Some(ref item_key) = filters.item_key {
            push_condition!("item_key = ", item_key.clone());
        }
        if let Some(ref statuses) = filters.statuses {
            if !statuses.is_empty() {
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push("status = ANY(")
                    .push_bind(statuses.clone())
                    .push(")");
                count_qb
                    .push("status = ANY(")
                    .push_bind(statuses.clone())
                    .push(")");
            }
        }
        if let Some(ref enqueue_source) = filters.enqueue_source {
            push_condition!("enqueue_source = ", enqueue_source.clone());
        }
        if let Some(requested_by_identity) = filters.requested_by_identity {
            push_condition!("requested_by_identity = ", requested_by_identity);
        }
        if let Some(requested_by_execution) = filters.requested_by_execution {
            push_condition!("requested_by_execution = ", requested_by_execution);
        }
        if let Some(requested_by_enforcement) = filters.requested_by_enforcement {
            push_condition!("requested_by_enforcement = ", requested_by_enforcement);
        }
        if let Some(leased_execution) = filters.leased_execution {
            push_condition!("leased_execution = ", leased_execution);
        }
        if let Some(lease_token) = filters.lease_token {
            push_condition!("lease_token = ", lease_token);
        }
        if filters.only_expired_leases {
            let now = filters.now.unwrap_or_else(Utc::now);
            if !has_where {
                qb.push(" WHERE ");
                count_qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
                count_qb.push(" AND ");
            }
            qb.push("status = 'leased' AND lease_expires_at IS NOT NULL AND lease_expires_at <= ")
                .push_bind(now);
            count_qb
                .push("status = 'leased' AND lease_expires_at IS NOT NULL AND lease_expires_at <= ")
                .push_bind(now);
        }

        let _ = has_where;

        let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;

        qb.push(" ORDER BY priority DESC, created ASC, id ASC LIMIT ")
            .push_bind(filters.limit as i64)
            .push(" OFFSET ")
            .push_bind(filters.offset as i64);

        let rows = qb.build_query_as::<WorkQueueItem>().fetch_all(db).await?;

        Ok(WorkQueueItemSearchResult {
            rows,
            total: total.max(0) as u64,
        })
    }

    pub async fn list_for_queue<'e, E>(executor: E, queue: Id) -> Result<Vec<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_item WHERE queue = $1 ORDER BY priority DESC, created ASC, id ASC",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(queue)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_queue_and_id<'e, E>(
        executor: E,
        queue: Id,
        id: Id,
    ) -> Result<Option<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_item WHERE queue = $1 AND id = $2",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(queue)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_pending_by_item_key<'e, E>(
        executor: E,
        queue: Id,
        item_key: &str,
    ) -> Result<Vec<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_item \
             WHERE queue = $1 AND item_key = $2 AND status = ANY($3::work_queue_item_status_enum[]) \
             ORDER BY created ASC, id ASC",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );
        let pending = vec![WorkQueueItemStatus::Queued, WorkQueueItemStatus::Retry];
        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(queue)
            .bind(item_key)
            .bind(pending)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn list_by_lease_token<'e, E>(
        executor: E,
        lease_token: Uuid,
    ) -> Result<Vec<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_item WHERE lease_token = $1 ORDER BY created ASC, id ASC",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(lease_token)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn delete_if_statuses<'e, E>(
        executor: E,
        id: Id,
        statuses: &[WorkQueueItemStatus],
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            "DELETE FROM work_queue_item WHERE id = $1 AND status = ANY($2::work_queue_item_status_enum[])",
        )
        .bind(id)
        .bind(statuses)
        .execute(executor)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_if_statuses<'e, E>(
        executor: E,
        id: Id,
        statuses: &[WorkQueueItemStatus],
        input: UpdateWorkQueueItemInput,
    ) -> Result<Option<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let allowed_statuses = if statuses.is_empty() {
            None
        } else {
            Some(statuses)
        };
        Self::update_internal(executor, id, allowed_statuses, input).await
    }

    pub async fn lease_next_batch<'e, E>(
        executor: E,
        input: LeaseWorkQueueItemsInput,
    ) -> Result<Vec<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if input.limit <= 0 {
            return Ok(Vec::new());
        }

        let coalescing = input
            .batch_coalescing
            .as_ref()
            .filter(|config| config.enabled);
        let coalescing_path_segments = coalescing
            .and_then(|config| config.group_by_path.as_deref())
            .map(|path| {
                path.split('.')
                    .filter(|segment| !segment.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|segments| !segments.is_empty())
            .unwrap_or_else(|| vec!["__disabled__".to_string()]);
        let coalescing_enabled = coalescing.is_some();
        let across_priorities = coalescing.is_some_and(|config| config.across_priorities);

        let returning_columns = format!(
            "item.{}",
            WORK_QUEUE_ITEM_SELECT_COLUMNS.replace(", ", ", item.")
        );
        let query = format!(
            r#"
            WITH anchor AS (
                SELECT id,
                       priority,
                       payload #> $7::text[] AS group_value
                FROM work_queue_item
                WHERE queue = $1
                  AND status = ANY($2::work_queue_item_status_enum[])
                ORDER BY priority DESC, created ASC, id ASC
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            ),
            extra_candidates_locked AS (
                SELECT item.id,
                       item.priority,
                       item.created
                FROM work_queue_item AS item
                CROSS JOIN anchor
                WHERE item.queue = $1
                  AND item.status = ANY($2::work_queue_item_status_enum[])
                  AND item.id <> anchor.id
                  AND (
                        $8 = false
                        OR anchor.group_value IS NULL
                        OR anchor.group_value = 'null'::jsonb
                        OR (
                            item.payload #> $7::text[] = anchor.group_value
                            AND ($9 OR item.priority = anchor.priority)
                        )
                  )
                ORDER BY item.priority DESC, item.created ASC, item.id ASC
                LIMIT GREATEST($3 - 1, 0)
                FOR UPDATE SKIP LOCKED
            ),
            extra_candidates AS (
                SELECT id,
                       ROW_NUMBER() OVER (ORDER BY priority DESC, created ASC, id ASC) AS sort_order
                FROM extra_candidates_locked
            ),
            candidate AS (
                SELECT id, 0 AS sort_order
                FROM anchor
                UNION ALL
                SELECT id, sort_order
                FROM extra_candidates
            ),
            updated AS (
                UPDATE work_queue_item AS item
                SET status = 'leased'::work_queue_item_status_enum,
                    leased_execution = $4,
                    lease_token = $5,
                    lease_expires_at = $6,
                    attempt_count = item.attempt_count + 1,
                    updated = NOW()
                FROM candidate
                WHERE item.id = candidate.id
                RETURNING candidate.sort_order, {}
            )
            SELECT {}
            FROM updated AS item
            ORDER BY item.sort_order ASC
            "#,
            returning_columns, WORK_QUEUE_ITEM_SELECT_COLUMNS
        );

        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(input.queue)
            .bind(input.ready_statuses)
            .bind(input.limit)
            .bind(input.leased_execution)
            .bind(input.lease_token)
            .bind(input.lease_expires_at)
            .bind(coalescing_path_segments)
            .bind(coalescing_enabled)
            .bind(across_priorities)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn attach_execution_to_lease<'e, E>(
        executor: E,
        lease_token: Uuid,
        execution_id: Id,
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            "UPDATE work_queue_item SET leased_execution = $2, updated = NOW() WHERE lease_token = $1 AND status = 'leased'",
        )
        .bind(lease_token)
        .bind(execution_id)
        .execute(executor)
        .await?;

        Ok(result.rows_affected())
    }

    pub async fn release_lease<'e, E>(
        executor: E,
        input: ReleaseWorkQueueLeaseInput,
    ) -> Result<Vec<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "UPDATE work_queue_item \
             SET status = $2, leased_execution = $3, lease_token = NULL, lease_expires_at = NULL, \
                 last_error = $4, ack_summary = $5, updated = NOW() \
             WHERE lease_token = $1 AND status = 'leased' \
             RETURNING {}",
            WORK_QUEUE_ITEM_SELECT_COLUMNS
        );

        sqlx::query_as::<_, WorkQueueItem>(&query)
            .bind(input.lease_token)
            .bind(input.new_status)
            .bind(input.leased_execution)
            .bind(&input.last_error)
            .bind(&input.ack_summary)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn reclaim_expired_leases<'e, E>(
        executor: E,
        now: DateTime<Utc>,
        queue: Option<Id>,
        reset_status: WorkQueueItemStatus,
    ) -> Result<Vec<WorkQueueItem>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("UPDATE work_queue_item SET status = ");
        qb.push_bind(reset_status)
            .push(", leased_execution = NULL, lease_token = NULL, lease_expires_at = NULL, updated = NOW() WHERE status = 'leased' AND lease_expires_at IS NOT NULL AND lease_expires_at <= ")
            .push_bind(now);
        if let Some(queue) = queue {
            qb.push(" AND queue = ").push_bind(queue);
        }
        qb.push(" RETURNING ").push(WORK_QUEUE_ITEM_SELECT_COLUMNS);

        qb.build_query_as::<WorkQueueItem>()
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

// ============================================================================
// WorkQueueDispatchRepository
// ============================================================================

#[derive(Debug, Clone, Default)]
pub struct WorkQueueDispatchSearchFilters {
    pub queue: Option<Id>,
    pub queue_ref: Option<String>,
    pub execution: Option<Id>,
    pub statuses: Option<Vec<WorkQueueDispatchStatus>>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug)]
pub struct WorkQueueDispatchSearchResult {
    pub rows: Vec<WorkQueueDispatch>,
    pub total: u64,
}

#[derive(Debug, Clone)]
pub struct CreateWorkQueueDispatchInput {
    pub id: Option<Id>,
    pub queue: Id,
    pub queue_ref: String,
    pub execution: Id,
    pub status: WorkQueueDispatchStatus,
    pub leased_item_count: i32,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateWorkQueueDispatchInput {
    pub status: Option<WorkQueueDispatchStatus>,
    pub leased_item_count: Option<i32>,
}

pub struct WorkQueueDispatchRepository;

impl Repository for WorkQueueDispatchRepository {
    type Entity = WorkQueueDispatch;

    fn table_name() -> &'static str {
        "work_queue_dispatch"
    }
}

#[async_trait::async_trait]
impl FindById for WorkQueueDispatchRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_dispatch WHERE id = $1",
            WORK_QUEUE_DISPATCH_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueDispatch>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for WorkQueueDispatchRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_dispatch ORDER BY created DESC LIMIT 1000",
            WORK_QUEUE_DISPATCH_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueDispatch>(&query)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for WorkQueueDispatchRepository {
    type CreateInput = CreateWorkQueueDispatchInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = if input.id.is_some() {
            format!(
                "INSERT INTO work_queue_dispatch (id, queue, queue_ref, execution, status, leased_item_count) \
                 VALUES ($1, $2, $3, $4, $5, $6) RETURNING {}",
                WORK_QUEUE_DISPATCH_SELECT_COLUMNS
            )
        } else {
            format!(
                "INSERT INTO work_queue_dispatch (queue, queue_ref, execution, status, leased_item_count) \
                 VALUES ($1, $2, $3, $4, $5) RETURNING {}",
                WORK_QUEUE_DISPATCH_SELECT_COLUMNS
            )
        };

        let mut query = sqlx::query_as::<_, WorkQueueDispatch>(&query);
        if let Some(id) = input.id {
            query = query.bind(id);
        }
        query
            .bind(input.queue)
            .bind(&input.queue_ref)
            .bind(input.execution)
            .bind(input.status)
            .bind(input.leased_item_count)
            .fetch_one(executor)
            .await
            .map_err(|e| {
                if let sqlx::Error::Database(ref db_err) = e {
                    if db_err.is_unique_violation() {
                        return Error::already_exists(
                            "WorkQueueDispatch",
                            "execution",
                            input.execution.to_string(),
                        );
                    }
                }
                e.into()
            })
    }
}

#[async_trait::async_trait]
impl Update for WorkQueueDispatchRepository {
    type UpdateInput = UpdateWorkQueueDispatchInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query = QueryBuilder::new("UPDATE work_queue_dispatch SET ");
        let mut has_updates = false;

        if let Some(status) = input.status {
            query.push("status = ").push_bind(status);
            has_updates = true;
        }
        if let Some(leased_item_count) = input.leased_item_count {
            if has_updates {
                query.push(", ");
            }
            query
                .push("leased_item_count = ")
                .push_bind(leased_item_count);
            has_updates = true;
        }

        if !has_updates {
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query
            .push(" RETURNING ")
            .push(WORK_QUEUE_DISPATCH_SELECT_COLUMNS);

        query
            .build_query_as::<WorkQueueDispatch>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for WorkQueueDispatchRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM work_queue_dispatch WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl WorkQueueDispatchRepository {
    pub async fn list_by_executions<'e, E>(
        executor: E,
        execution_ids: &[Id],
    ) -> Result<Vec<WorkQueueDispatch>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if execution_ids.is_empty() {
            return Ok(Vec::new());
        }

        let query = format!(
            "SELECT {} FROM work_queue_dispatch \
             WHERE execution = ANY($1) \
             ORDER BY created ASC, id ASC",
            WORK_QUEUE_DISPATCH_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueDispatch>(&query)
            .bind(execution_ids)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_execution<'e, E>(
        executor: E,
        execution: Id,
    ) -> Result<Option<WorkQueueDispatch>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM work_queue_dispatch WHERE execution = $1",
            WORK_QUEUE_DISPATCH_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueDispatch>(&query)
            .bind(execution)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn list_active<'e, E>(executor: E) -> Result<Vec<WorkQueueDispatch>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let active = vec![
            WorkQueueDispatchStatus::Leased,
            WorkQueueDispatchStatus::Dispatched,
        ];
        let query = format!(
            "SELECT {} FROM work_queue_dispatch WHERE status = ANY($1::work_queue_dispatch_status_enum[]) ORDER BY created ASC",
            WORK_QUEUE_DISPATCH_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueDispatch>(&query)
            .bind(active)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn latest_terminal_for_queue<'e, E>(
        executor: E,
        queue: Id,
    ) -> Result<Option<WorkQueueDispatch>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let terminal = vec![
            WorkQueueDispatchStatus::Completed,
            WorkQueueDispatchStatus::Failed,
            WorkQueueDispatchStatus::Released,
            WorkQueueDispatchStatus::Cancelled,
        ];
        let query = format!(
            "SELECT {} FROM work_queue_dispatch \
             WHERE queue = $1 AND status = ANY($2::work_queue_dispatch_status_enum[]) \
             ORDER BY updated DESC LIMIT 1",
            WORK_QUEUE_DISPATCH_SELECT_COLUMNS
        );
        sqlx::query_as::<_, WorkQueueDispatch>(&query)
            .bind(queue)
            .bind(terminal)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn search<'e, E>(
        db: E,
        filters: &WorkQueueDispatchSearchFilters,
    ) -> Result<WorkQueueDispatchSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let mut qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(format!(
            "SELECT {} FROM work_queue_dispatch",
            WORK_QUEUE_DISPATCH_SELECT_COLUMNS
        ));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM work_queue_dispatch");
        let mut has_where = false;

        macro_rules! push_condition {
            ($sql:expr, $value:expr) => {{
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push($sql).push_bind($value.clone());
                count_qb.push($sql).push_bind($value);
            }};
        }

        if let Some(queue) = filters.queue {
            push_condition!("queue = ", queue);
        }
        if let Some(ref queue_ref) = filters.queue_ref {
            push_condition!("queue_ref = ", queue_ref.clone());
        }
        if let Some(execution) = filters.execution {
            push_condition!("execution = ", execution);
        }
        if let Some(ref statuses) = filters.statuses {
            if !statuses.is_empty() {
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push("status = ANY(")
                    .push_bind(statuses.clone())
                    .push(")");
                count_qb
                    .push("status = ANY(")
                    .push_bind(statuses.clone())
                    .push(")");
            }
        }

        let _ = has_where;

        let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;

        qb.push(" ORDER BY created DESC LIMIT ")
            .push_bind(filters.limit as i64)
            .push(" OFFSET ")
            .push_bind(filters.offset as i64);

        let rows = qb
            .build_query_as::<WorkQueueDispatch>()
            .fetch_all(db)
            .await?;

        Ok(WorkQueueDispatchSearchResult {
            rows,
            total: total.max(0) as u64,
        })
    }
}
