//! Trigger and Sensor repository for database operations
//!
//! This module provides CRUD operations and queries for Trigger and Sensor entities.

use crate::models::{
    enums::ActionReferenceVisibility, trigger::*, Id, JsonDict, JsonSchema, RetentionPolicyType,
};
use crate::{Error, Result};
use serde_json::Value as JsonValue;
use sqlx::{Executor, Postgres, QueryBuilder};

use super::{Create, Delete, FindById, FindByRef, List, Patch, Repository, Update};

/// Columns selected in all Trigger queries. Must match the `Trigger` model's `FromRow` fields.
pub const TRIGGER_COLUMNS: &str = "id, ref, pack, pack_ref, label, description, enabled, \
    param_schema, out_schema, webhook_enabled, webhook_key, webhook_config, \
    sensor, sensor_ref, is_adhoc, reference_visibility, reference_allowed_pack_refs, \
    created, updated";

// ============================================================================
// Trigger Search
// ============================================================================

/// Filters for [`TriggerRepository::list_search`].
///
/// All fields are optional and combinable (AND). Pagination is always applied.
#[derive(Debug, Clone, Default)]
pub struct TriggerSearchFilters {
    /// Filter by pack ID
    pub pack: Option<Id>,
    /// Filter by sensor ID
    pub sensor: Option<Id>,
    /// Filter by enabled status
    pub enabled: Option<bool>,
    /// Row-visibility predicate pushed down into SQL. `None` skips all
    /// visibility filtering (for internal/system callers); API routes must
    /// always populate this so totals and pagination stay consistent with
    /// per-row read access.
    pub visibility: Option<TriggerVisibilityFilter>,
    pub limit: u32,
    pub offset: u32,
}

/// Row-visibility scope for trigger list/search, derived once per request
/// from the identity's effective RBAC grants (see
/// `crates/api/src/routes/triggers.rs::compute_trigger_read_scope`) plus an
/// optional cross-pack "referencing pack" check mirroring
/// [`crate::action_visibility::trigger_reference_allowed`].
///
/// A row is visible when ANY of the following hold:
/// - `reference_visibility` is `public`;
/// - `referencing_pack_ref` is set and the trigger's `reference_visibility`
///   allows that pack to reference it (owning-pack OR allow-listed pack for
///   `restricted`, owning-pack only for `private`);
/// - the identity holds an unconstrained grant (`unscoped`), or the
///   trigger's `pack_ref`/`ref`/`id` matches one of the allow-lists derived
///   from the identity's grants.
#[derive(Debug, Clone, Default)]
pub struct TriggerVisibilityFilter {
    /// True when the identity holds an unconstrained grant; no additional
    /// predicate is applied (all rows visible).
    pub unscoped: bool,
    pub allowed_pack_refs: Vec<String>,
    pub allowed_refs: Vec<String>,
    pub allowed_ids: Vec<Id>,
    /// Pack ref of a caller that wants to reference/subscribe to this
    /// trigger (e.g. a rule being authored in that pack), independent of the
    /// identity's own RBAC grants.
    pub referencing_pack_ref: Option<String>,
}

/// Result of [`TriggerRepository::list_search`].
#[derive(Debug)]
pub struct TriggerSearchResult {
    pub rows: Vec<Trigger>,
    pub total: u64,
}

/// Push the row-visibility predicate described by [`TriggerVisibilityFilter`]
/// into `qb`, wrapped in a single set of parentheses. Callers are
/// responsible for prefixing this with `WHERE`/`AND` and for skipping the
/// call entirely when `visibility.unscoped` is true (no predicate needed).
fn push_trigger_visibility_predicate(
    qb: &mut QueryBuilder<'_, Postgres>,
    visibility: &TriggerVisibilityFilter,
) {
    qb.push("(reference_visibility = ");
    qb.push_bind(ActionReferenceVisibility::Public);

    if let Some(referencing_pack_ref) = &visibility.referencing_pack_ref {
        qb.push(" OR (reference_visibility = ");
        qb.push_bind(ActionReferenceVisibility::Private);
        qb.push(" AND pack_ref = ");
        qb.push_bind(referencing_pack_ref.clone());
        qb.push(") OR (reference_visibility = ");
        qb.push_bind(ActionReferenceVisibility::Restricted);
        qb.push(" AND (pack_ref = ");
        qb.push_bind(referencing_pack_ref.clone());
        qb.push(" OR ");
        qb.push_bind(referencing_pack_ref.clone());
        qb.push(" = ANY(reference_allowed_pack_refs)))");
    }

    qb.push(" OR (pack_ref = ANY(");
    qb.push_bind(visibility.allowed_pack_refs.clone());
    qb.push(") OR ref = ANY(");
    qb.push_bind(visibility.allowed_refs.clone());
    qb.push(") OR id = ANY(");
    qb.push_bind(visibility.allowed_ids.clone());
    qb.push(")))");
}

// ============================================================================
// Sensor Search
// ============================================================================

/// Filters for [`SensorRepository::list_search`].
///
/// All fields are optional and combinable (AND). Pagination is always applied.
#[derive(Debug, Clone, Default)]
pub struct SensorSearchFilters {
    /// Filter by pack ID
    pub pack: Option<Id>,
    /// Filter by enabled status
    pub enabled: Option<bool>,
    /// Row-visibility predicate pushed down into SQL. `None` skips all
    /// visibility filtering (for internal/system callers); API routes must
    /// always populate this so totals and pagination stay consistent with
    /// per-row read access.
    pub visibility: Option<SensorVisibilityFilter>,
    pub limit: u32,
    pub offset: u32,
}

/// Row-visibility scope for sensor list/search, derived once per request
/// from the identity's effective RBAC grants (see
/// `crates/api/src/routes/triggers.rs::compute_sensor_read_scope`). Sensors
/// have no per-row reference-visibility concept, so this is a plain
/// pack/ref/id allow-list.
#[derive(Debug, Clone, Default)]
pub struct SensorVisibilityFilter {
    /// True when the identity holds an unconstrained grant; no additional
    /// predicate is applied (all rows visible).
    pub unscoped: bool,
    pub allowed_pack_refs: Vec<String>,
    pub allowed_refs: Vec<String>,
    pub allowed_ids: Vec<Id>,
}

/// Result of [`SensorRepository::list_search`].
#[derive(Debug)]
pub struct SensorSearchResult {
    pub rows: Vec<Sensor>,
    pub total: u64,
}

/// Repository for Trigger operations
pub struct TriggerRepository;

pub fn validate_trigger_reference_visibility_config(
    visibility: ActionReferenceVisibility,
    allowed_pack_refs: &[String],
) -> Result<()> {
    for pack_ref in allowed_pack_refs {
        crate::schema::RefValidator::validate_pack_ref(pack_ref)?;
    }

    if visibility != ActionReferenceVisibility::Restricted && !allowed_pack_refs.is_empty() {
        return Err(Error::validation(
            "reference_allowed_pack_refs may only be set when reference_visibility is restricted",
        ));
    }

    Ok(())
}

impl Repository for TriggerRepository {
    type Entity = Trigger;

    fn table_name() -> &'static str {
        "triggers"
    }
}

/// Input for creating a new trigger
#[derive(Debug, Clone)]
pub struct CreateTriggerInput {
    pub r#ref: String,
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub label: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub param_schema: Option<JsonSchema>,
    pub out_schema: Option<JsonSchema>,
    pub sensor: Option<Id>,
    pub sensor_ref: Option<String>,
    pub is_adhoc: bool,
    pub reference_visibility: ActionReferenceVisibility,
    pub reference_allowed_pack_refs: Vec<String>,
}

/// Input for updating a trigger
#[derive(Debug, Clone, Default)]
pub struct UpdateTriggerInput {
    pub label: Option<String>,
    pub description: Option<Patch<String>>,
    pub enabled: Option<bool>,
    pub param_schema: Option<Patch<JsonSchema>>,
    pub out_schema: Option<Patch<JsonSchema>>,
    pub sensor: Option<Patch<Id>>,
    pub sensor_ref: Option<Patch<String>>,
    pub reference_visibility: Option<ActionReferenceVisibility>,
    pub reference_allowed_pack_refs: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_restricted_trigger_allow_list() {
        assert!(validate_trigger_reference_visibility_config(
            ActionReferenceVisibility::Restricted,
            &["incident_response".to_string()]
        )
        .is_ok());
    }

    #[test]
    fn rejects_allow_list_for_non_restricted_triggers() {
        let err = validate_trigger_reference_visibility_config(
            ActionReferenceVisibility::Private,
            &["incident_response".to_string()],
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("reference_allowed_pack_refs may only be set"));
    }
}

#[async_trait::async_trait]
impl FindById for TriggerRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let trigger = sqlx::query_as::<_, Trigger>(&format!(
            "SELECT {} FROM trigger WHERE id = $1",
            TRIGGER_COLUMNS
        ))
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(trigger)
    }
}

#[async_trait::async_trait]
impl FindByRef for TriggerRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let trigger = sqlx::query_as::<_, Trigger>(&format!(
            "SELECT {} FROM trigger WHERE ref = $1",
            TRIGGER_COLUMNS
        ))
        .bind(ref_str)
        .fetch_optional(executor)
        .await?;

        Ok(trigger)
    }
}

#[async_trait::async_trait]
impl List for TriggerRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let triggers = sqlx::query_as::<_, Trigger>(&format!(
            "SELECT {} FROM trigger ORDER BY ref ASC",
            TRIGGER_COLUMNS
        ))
        .fetch_all(executor)
        .await?;

        Ok(triggers)
    }
}

#[async_trait::async_trait]
impl Create for TriggerRepository {
    type CreateInput = CreateTriggerInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        validate_trigger_reference_visibility_config(
            input.reference_visibility,
            &input.reference_allowed_pack_refs,
        )?;

        let trigger = sqlx::query_as::<_, Trigger>(
            r#"
            INSERT INTO trigger (ref, pack, pack_ref, label, description, enabled,
                                 param_schema, out_schema, sensor, sensor_ref, is_adhoc,
                                 reference_visibility, reference_allowed_pack_refs)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            RETURNING id, ref, pack, pack_ref, label, description, enabled,
                      param_schema, out_schema, webhook_enabled, webhook_key, webhook_config,
                      sensor, sensor_ref, is_adhoc, reference_visibility, reference_allowed_pack_refs,
                      created, updated
            "#,
        )
        .bind(&input.r#ref)
        .bind(input.pack)
        .bind(&input.pack_ref)
        .bind(&input.label)
        .bind(&input.description)
        .bind(input.enabled)
        .bind(&input.param_schema)
        .bind(&input.out_schema)
        .bind(input.sensor)
        .bind(&input.sensor_ref)
        .bind(input.is_adhoc)
        .bind(input.reference_visibility)
        .bind(&input.reference_allowed_pack_refs)
        .fetch_one(executor)
        .await
        .map_err(|e| {
            // Convert unique constraint violation to AlreadyExists error
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.is_unique_violation() {
                    return crate::Error::already_exists("Trigger", "ref", &input.r#ref);
                }
            }
            e.into()
        })?;

        Ok(trigger)
    }
}

#[async_trait::async_trait]
impl Update for TriggerRepository {
    type UpdateInput = UpdateTriggerInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
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
                crate::schema::RefValidator::validate_pack_ref(pack_ref)?;
            }
        }

        // Build update query

        let mut query = QueryBuilder::new("UPDATE trigger SET ");
        let mut has_updates = false;

        if let Some(label) = &input.label {
            query.push("label = ");
            query.push_bind(label);
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
            query.push("enabled = ");
            query.push_bind(enabled);
            has_updates = true;
        }

        if let Some(param_schema) = &input.param_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("param_schema = ");
            match param_schema {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<JsonSchema>::None),
            };
            has_updates = true;
        }

        if let Some(out_schema) = &input.out_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("out_schema = ");
            match out_schema {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<JsonSchema>::None),
            };
            has_updates = true;
        }

        if let Some(sensor) = &input.sensor {
            if has_updates {
                query.push(", ");
            }
            query.push("sensor = ");
            match sensor {
                Patch::Set(value) => query.push_bind(Some(*value)),
                Patch::Clear => query.push_bind(Option::<Id>::None),
            };
            has_updates = true;
        }

        if let Some(sensor_ref) = &input.sensor_ref {
            if has_updates {
                query.push(", ");
            }
            query.push("sensor_ref = ");
            match sensor_ref {
                Patch::Set(value) => query.push_bind(Some(value.clone())),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }

        if let Some(reference_visibility) = input.reference_visibility {
            if has_updates {
                query.push(", ");
            }
            query.push("reference_visibility = ");
            query.push_bind(reference_visibility);
            has_updates = true;
        }

        if let Some(reference_allowed_pack_refs) = &input.reference_allowed_pack_refs {
            if has_updates {
                query.push(", ");
            }
            query.push("reference_allowed_pack_refs = ");
            query.push_bind(reference_allowed_pack_refs);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ");
        query.push_bind(id);
        query.push(" RETURNING ");
        query.push(TRIGGER_COLUMNS);

        let trigger = query
            .build_query_as::<Trigger>()
            .fetch_one(executor)
            .await
            .map_err(|e| {
                // Convert RowNotFound to NotFound error
                if matches!(e, sqlx::Error::RowNotFound) {
                    return crate::Error::not_found("trigger", "id", id.to_string());
                }
                e.into()
            })?;

        Ok(trigger)
    }
}

#[async_trait::async_trait]
impl Delete for TriggerRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM trigger WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl TriggerRepository {
    /// Delete non-adhoc triggers belonging to a pack whose refs are NOT in the given set.
    ///
    /// Used during pack reinstallation to clean up triggers that were removed
    /// from the pack's YAML files. Ad-hoc (user-created) triggers are preserved.
    pub async fn delete_non_adhoc_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        keep_refs: &[String],
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = if keep_refs.is_empty() {
            sqlx::query("DELETE FROM trigger WHERE pack = $1 AND is_adhoc = false")
                .bind(pack_id)
                .execute(executor)
                .await?
        } else {
            sqlx::query(
                "DELETE FROM trigger WHERE pack = $1 AND is_adhoc = false AND ref != ALL($2)",
            )
            .bind(pack_id)
            .bind(keep_refs)
            .execute(executor)
            .await?
        };

        Ok(result.rows_affected())
    }

    /// Search triggers with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    pub async fn list_search<'e, E>(
        db: E,
        filters: &TriggerSearchFilters,
    ) -> Result<TriggerSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let select_cols = TRIGGER_COLUMNS;

        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {select_cols} FROM trigger"));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM trigger");

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

        if let Some(pack_id) = filters.pack {
            push_condition!("pack = ", pack_id);
        }
        if let Some(sensor_id) = filters.sensor {
            push_condition!("sensor = ", sensor_id);
        }
        if let Some(enabled) = filters.enabled {
            push_condition!("enabled = ", enabled);
        }

        if let Some(visibility) = &filters.visibility {
            if !visibility.unscoped {
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                push_trigger_visibility_predicate(&mut qb, visibility);
                push_trigger_visibility_predicate(&mut count_qb, visibility);
            }
        }

        // Suppress unused-assignment warning from the macro's last expansion.
        let _ = has_where;

        // Count
        let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;
        let total = total.max(0) as u64;

        // Data query
        qb.push(" ORDER BY ref ASC");
        qb.push(" LIMIT ");
        qb.push_bind(filters.limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let rows: Vec<Trigger> = qb.build_query_as().fetch_all(db).await?;

        Ok(TriggerSearchResult { rows, total })
    }

    /// Find triggers by pack ID
    pub async fn find_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Vec<Trigger>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let triggers = sqlx::query_as::<_, Trigger>(&format!(
            "SELECT {} FROM trigger WHERE pack = $1 ORDER BY ref ASC",
            TRIGGER_COLUMNS
        ))
        .bind(pack_id)
        .fetch_all(executor)
        .await?;

        Ok(triggers)
    }

    /// Find enabled triggers
    pub async fn find_enabled<'e, E>(executor: E) -> Result<Vec<Trigger>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let triggers = sqlx::query_as::<_, Trigger>(&format!(
            "SELECT {} FROM trigger WHERE enabled = true ORDER BY ref ASC",
            TRIGGER_COLUMNS
        ))
        .fetch_all(executor)
        .await?;

        Ok(triggers)
    }

    /// Find triggers that belong to a specific sensor
    pub async fn find_by_sensor<'e, E>(executor: E, sensor_id: Id) -> Result<Vec<Trigger>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let triggers = sqlx::query_as::<_, Trigger>(&format!(
            "SELECT {} FROM trigger WHERE sensor = $1 ORDER BY ref ASC",
            TRIGGER_COLUMNS
        ))
        .bind(sensor_id)
        .fetch_all(executor)
        .await?;

        Ok(triggers)
    }

    /// Find triggers that belong to a specific sensor by sensor ref
    pub async fn find_by_sensor_ref<'e, E>(executor: E, sensor_ref: &str) -> Result<Vec<Trigger>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let triggers = sqlx::query_as::<_, Trigger>(&format!(
            "SELECT {} FROM trigger WHERE sensor_ref = $1 ORDER BY ref ASC",
            TRIGGER_COLUMNS
        ))
        .bind(sensor_ref)
        .fetch_all(executor)
        .await?;

        Ok(triggers)
    }

    /// Find trigger by webhook key
    pub async fn find_by_webhook_key<'e, E>(
        executor: E,
        webhook_key: &str,
    ) -> Result<Option<Trigger>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let trigger = sqlx::query_as::<_, Trigger>(&format!(
            "SELECT {} FROM trigger WHERE webhook_key = $1",
            TRIGGER_COLUMNS
        ))
        .bind(webhook_key)
        .fetch_optional(executor)
        .await?;

        Ok(trigger)
    }

    /// Enable webhooks for a trigger
    pub async fn enable_webhook<'e, E>(executor: E, trigger_id: Id) -> Result<WebhookInfo>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        #[derive(sqlx::FromRow)]
        struct WebhookResult {
            webhook_enabled: bool,
            webhook_key: String,
            webhook_url: String,
        }

        let result = sqlx::query_as::<_, WebhookResult>(
            r#"
            SELECT * FROM enable_trigger_webhook($1)
            "#,
        )
        .bind(trigger_id)
        .fetch_one(executor)
        .await?;

        Ok(WebhookInfo {
            enabled: result.webhook_enabled,
            webhook_key: result.webhook_key,
            webhook_url: result.webhook_url,
        })
    }

    /// Disable webhooks for a trigger
    pub async fn disable_webhook<'e, E>(executor: E, trigger_id: Id) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT disable_trigger_webhook($1)
            "#,
        )
        .bind(trigger_id)
        .fetch_one(executor)
        .await?;

        Ok(result)
    }

    /// Regenerate webhook key for a trigger
    pub async fn regenerate_webhook_key<'e, E>(
        executor: E,
        trigger_id: Id,
    ) -> Result<WebhookKeyRegenerate>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        #[derive(sqlx::FromRow)]
        struct RegenerateResult {
            webhook_key: String,
            previous_key_revoked: bool,
        }

        let result = sqlx::query_as::<_, RegenerateResult>(
            r#"
            SELECT * FROM regenerate_trigger_webhook_key($1)
            "#,
        )
        .bind(trigger_id)
        .fetch_one(executor)
        .await?;

        Ok(WebhookKeyRegenerate {
            webhook_key: result.webhook_key,
            previous_key_revoked: result.previous_key_revoked,
        })
    }

    // ========================================================================
    // Phase 3: Advanced Webhook Features
    // ========================================================================

    /// Update webhook configuration for a trigger
    pub async fn update_webhook_config<'e, E>(
        executor: E,
        trigger_id: Id,
        config: serde_json::Value,
    ) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query(
            r#"
            UPDATE trigger
            SET webhook_config = $2, updated = NOW()
            WHERE id = $1
            "#,
        )
        .bind(trigger_id)
        .bind(config)
        .execute(executor)
        .await?;

        Ok(())
    }

    /// Log webhook event for auditing and analytics
    pub async fn log_webhook_event<'e, E>(executor: E, input: WebhookEventLogInput) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO webhook_event_log (
                trigger_id, trigger_ref, webhook_key, event_id,
                source_ip, user_agent, payload_size_bytes, headers,
                status_code, error_message, processing_time_ms,
                hmac_verified, rate_limited, ip_allowed
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            RETURNING id
            "#,
        )
        .bind(input.trigger_id)
        .bind(input.trigger_ref)
        .bind(input.webhook_key)
        .bind(input.event_id)
        .bind(input.source_ip)
        .bind(input.user_agent)
        .bind(input.payload_size_bytes)
        .bind(input.headers)
        .bind(input.status_code)
        .bind(input.error_message)
        .bind(input.processing_time_ms)
        .bind(input.hmac_verified)
        .bind(input.rate_limited)
        .bind(input.ip_allowed)
        .fetch_one(executor)
        .await?;

        Ok(id)
    }

    /// Count non-rate-limited webhook requests within a recent fixed window.
    ///
    /// This is intentionally database-backed so rate limits are enforced
    /// consistently across multiple API instances.
    pub async fn count_recent_webhook_requests<'e, E>(
        executor: E,
        trigger_id: Id,
        webhook_key: &str,
        source_ip: Option<&str>,
        window_seconds: i32,
    ) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let count = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM webhook_event_log
            WHERE trigger_id = $1
              AND webhook_key = $2
              AND source_ip IS NOT DISTINCT FROM $3
              AND rate_limited = FALSE
              AND created >= NOW() - make_interval(secs => $4)
            "#,
        )
        .bind(trigger_id)
        .bind(webhook_key)
        .bind(source_ip)
        .bind(window_seconds)
        .fetch_one(executor)
        .await?;

        Ok(count)
    }
}

/// Webhook information returned when enabling webhooks
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookInfo {
    pub enabled: bool,
    pub webhook_key: String,
    pub webhook_url: String,
}

/// Webhook key regeneration result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookKeyRegenerate {
    pub webhook_key: String,
    pub previous_key_revoked: bool,
}

/// Input for logging webhook events
#[derive(Debug, Clone)]
pub struct WebhookEventLogInput {
    pub trigger_id: Id,
    pub trigger_ref: String,
    pub webhook_key: String,
    pub event_id: Option<Id>,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
    pub payload_size_bytes: Option<i32>,
    pub headers: Option<JsonValue>,
    pub status_code: i32,
    pub error_message: Option<String>,
    pub processing_time_ms: Option<i32>,
    pub hmac_verified: Option<bool>,
    pub rate_limited: bool,
    pub ip_allowed: Option<bool>,
}

// ============================================================================
// Sensor Repository
// ============================================================================

/// Repository for Sensor operations
pub struct SensorRepository;

impl Repository for SensorRepository {
    type Entity = Sensor;

    fn table_name() -> &'static str {
        "sensor"
    }
}

const SENSOR_SELECT_COLUMNS: &str = "id, ref, pack, pack_ref, label, description, entrypoint, \
     runtime, runtime_ref, runtime_version_constraint, enabled, param_schema, config, \
     worker_selector, worker_tolerations, worker_affinity, log_retention_policy, \
     log_retention_limit, artifact_retention_policy, artifact_retention_limit, created, updated";

fn validate_log_retention_limit(limit: i32) -> Result<()> {
    if limit <= 0 {
        return Err(crate::Error::validation(
            "log_retention_limit must be greater than zero",
        ));
    }
    Ok(())
}

/// Input for creating a new sensor
#[derive(Debug, Clone)]
pub struct CreateSensorInput {
    pub r#ref: String,
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub label: String,
    pub description: Option<String>,
    pub entrypoint: String,
    pub runtime: Id,
    pub runtime_ref: String,
    pub runtime_version_constraint: Option<String>,
    pub enabled: bool,
    pub param_schema: Option<JsonSchema>,
    pub config: Option<JsonValue>,
    pub worker_selector: JsonDict,
    pub worker_tolerations: JsonDict,
    pub worker_affinity: JsonDict,
    pub log_retention_policy: Option<RetentionPolicyType>,
    pub log_retention_limit: Option<i32>,
    pub artifact_retention_policy: Option<RetentionPolicyType>,
    pub artifact_retention_limit: Option<i32>,
}

/// Input for updating a sensor
#[derive(Debug, Clone, Default)]
pub struct UpdateSensorInput {
    pub label: Option<String>,
    pub description: Option<Patch<String>>,
    pub entrypoint: Option<String>,
    pub runtime: Option<Id>,
    pub runtime_ref: Option<String>,
    pub runtime_version_constraint: Option<Patch<String>>,
    pub enabled: Option<bool>,
    pub param_schema: Option<Patch<JsonSchema>>,
    pub config: Option<JsonValue>,
    pub worker_selector: Option<JsonDict>,
    pub worker_tolerations: Option<JsonDict>,
    pub worker_affinity: Option<JsonDict>,
    pub log_retention_policy: Option<Patch<RetentionPolicyType>>,
    pub log_retention_limit: Option<Patch<i32>>,
    pub artifact_retention_policy: Option<Patch<RetentionPolicyType>>,
    pub artifact_retention_limit: Option<Patch<i32>>,
}

#[async_trait::async_trait]
impl FindById for SensorRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sensor = sqlx::query_as::<_, Sensor>(&format!(
            "SELECT {SENSOR_SELECT_COLUMNS} FROM sensor WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(sensor)
    }
}

#[async_trait::async_trait]
impl FindByRef for SensorRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sensor = sqlx::query_as::<_, Sensor>(&format!(
            "SELECT {SENSOR_SELECT_COLUMNS} FROM sensor WHERE ref = $1"
        ))
        .bind(ref_str)
        .fetch_optional(executor)
        .await?;

        Ok(sensor)
    }
}

#[async_trait::async_trait]
impl List for SensorRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sensors = sqlx::query_as::<_, Sensor>(&format!(
            "SELECT {SENSOR_SELECT_COLUMNS} FROM sensor ORDER BY ref ASC"
        ))
        .fetch_all(executor)
        .await?;

        Ok(sensors)
    }
}

#[async_trait::async_trait]
impl Create for SensorRepository {
    type CreateInput = CreateSensorInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if let Some(limit) = input.log_retention_limit {
            validate_log_retention_limit(limit)?;
        }
        if let Some(limit) = input.artifact_retention_limit {
            validate_log_retention_limit(limit)?;
        }

        let sensor = sqlx::query_as::<_, Sensor>(&format!(
            "INSERT INTO sensor (ref, pack, pack_ref, label, description, entrypoint, \
                 runtime, runtime_ref, runtime_version_constraint, enabled, param_schema, config, \
                 worker_selector, worker_tolerations, worker_affinity, log_retention_policy, \
                 log_retention_limit, artifact_retention_policy, artifact_retention_limit) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19) \
                 RETURNING {SENSOR_SELECT_COLUMNS}"
        ))
        .bind(&input.r#ref)
        .bind(input.pack)
        .bind(&input.pack_ref)
        .bind(&input.label)
        .bind(&input.description)
        .bind(&input.entrypoint)
        .bind(input.runtime)
        .bind(&input.runtime_ref)
        .bind(&input.runtime_version_constraint)
        .bind(input.enabled)
        .bind(&input.param_schema)
        .bind(&input.config)
        .bind(&input.worker_selector)
        .bind(&input.worker_tolerations)
        .bind(&input.worker_affinity)
        .bind(input.log_retention_policy)
        .bind(input.log_retention_limit)
        .bind(input.artifact_retention_policy)
        .bind(input.artifact_retention_limit)
        .fetch_one(executor)
        .await?;

        Ok(sensor)
    }
}

#[async_trait::async_trait]
impl Update for SensorRepository {
    type UpdateInput = UpdateSensorInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if let Some(Patch::Set(limit)) = &input.log_retention_limit {
            validate_log_retention_limit(*limit)?;
        }
        if let Some(Patch::Set(limit)) = &input.artifact_retention_limit {
            validate_log_retention_limit(*limit)?;
        }

        // Build update query

        let mut query = QueryBuilder::new("UPDATE sensor SET ");
        let mut has_updates = false;

        if let Some(label) = &input.label {
            query.push("label = ");
            query.push_bind(label);
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

        if let Some(entrypoint) = &input.entrypoint {
            if has_updates {
                query.push(", ");
            }
            query.push("entrypoint = ");
            query.push_bind(entrypoint);
            has_updates = true;
        }

        if let Some(enabled) = input.enabled {
            if has_updates {
                query.push(", ");
            }
            query.push("enabled = ");
            query.push_bind(enabled);
            has_updates = true;
        }

        if let Some(runtime) = input.runtime {
            if has_updates {
                query.push(", ");
            }
            query.push("runtime = ");
            query.push_bind(runtime);
            has_updates = true;
        }

        if let Some(runtime_ref) = &input.runtime_ref {
            if has_updates {
                query.push(", ");
            }
            query.push("runtime_ref = ");
            query.push_bind(runtime_ref);
            has_updates = true;
        }

        if let Some(runtime_version_constraint) = &input.runtime_version_constraint {
            if has_updates {
                query.push(", ");
            }
            query.push("runtime_version_constraint = ");
            match runtime_version_constraint {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }

        if let Some(param_schema) = &input.param_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("param_schema = ");
            match param_schema {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<JsonSchema>::None),
            };
            has_updates = true;
        }

        if let Some(config) = &input.config {
            if has_updates {
                query.push(", ");
            }
            query.push("config = ");
            query.push_bind(config);
            has_updates = true;
        }

        if let Some(worker_selector) = &input.worker_selector {
            if has_updates {
                query.push(", ");
            }
            query.push("worker_selector = ");
            query.push_bind(worker_selector);
            has_updates = true;
        }

        if let Some(worker_tolerations) = &input.worker_tolerations {
            if has_updates {
                query.push(", ");
            }
            query.push("worker_tolerations = ");
            query.push_bind(worker_tolerations);
            has_updates = true;
        }

        if let Some(worker_affinity) = &input.worker_affinity {
            if has_updates {
                query.push(", ");
            }
            query.push("worker_affinity = ");
            query.push_bind(worker_affinity);
            has_updates = true;
        }

        if let Some(log_retention_policy) = input.log_retention_policy {
            if has_updates {
                query.push(", ");
            }
            query.push("log_retention_policy = ");
            match log_retention_policy {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<RetentionPolicyType>::None),
            };
            has_updates = true;
        }

        if let Some(log_retention_limit) = input.log_retention_limit {
            if has_updates {
                query.push(", ");
            }
            query.push("log_retention_limit = ");
            match log_retention_limit {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<i32>::None),
            };
            has_updates = true;
        }

        if let Some(artifact_retention_policy) = input.artifact_retention_policy {
            if has_updates {
                query.push(", ");
            }
            query.push("artifact_retention_policy = ");
            match artifact_retention_policy {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<RetentionPolicyType>::None),
            };
            has_updates = true;
        }

        if let Some(artifact_retention_limit) = input.artifact_retention_limit {
            if has_updates {
                query.push(", ");
            }
            query.push("artifact_retention_limit = ");
            match artifact_retention_limit {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<i32>::None),
            };
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ");
        query.push_bind(id);
        query.push(" RETURNING ");
        query.push(SENSOR_SELECT_COLUMNS);

        let sensor = query.build_query_as::<Sensor>().fetch_one(executor).await?;

        Ok(sensor)
    }
}

#[async_trait::async_trait]
impl Delete for SensorRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM sensor WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

/// Push a `(pack_ref = ANY($) OR ref = ANY($) OR id = ANY($))` predicate
/// wrapped in a single set of parentheses. Callers are responsible for
/// prefixing this with `WHERE`/`AND` and for skipping the call entirely when
/// `visibility.unscoped` is true (no predicate needed).
fn push_sensor_visibility_predicate(
    qb: &mut QueryBuilder<'_, Postgres>,
    visibility: &SensorVisibilityFilter,
) {
    qb.push("(pack_ref = ANY(");
    qb.push_bind(visibility.allowed_pack_refs.clone());
    qb.push(") OR ref = ANY(");
    qb.push_bind(visibility.allowed_refs.clone());
    qb.push(") OR id = ANY(");
    qb.push_bind(visibility.allowed_ids.clone());
    qb.push("))");
}

impl SensorRepository {
    /// Delete non-adhoc sensors belonging to a pack whose refs are NOT in the given set.
    ///
    /// Used during pack reinstallation to clean up sensors that were removed
    /// from the pack's YAML files.
    pub async fn delete_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        keep_refs: &[String],
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = if keep_refs.is_empty() {
            sqlx::query("DELETE FROM sensor WHERE pack = $1")
                .bind(pack_id)
                .execute(executor)
                .await?
        } else {
            sqlx::query("DELETE FROM sensor WHERE pack = $1 AND ref != ALL($2)")
                .bind(pack_id)
                .bind(keep_refs)
                .execute(executor)
                .await?
        };

        Ok(result.rows_affected())
    }

    /// Search sensors with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    pub async fn list_search<'e, E>(
        db: E,
        filters: &SensorSearchFilters,
    ) -> Result<SensorSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let select_cols = SENSOR_SELECT_COLUMNS;

        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {select_cols} FROM sensor"));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM sensor");

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

        if let Some(pack_id) = filters.pack {
            push_condition!("pack = ", pack_id);
        }
        if let Some(enabled) = filters.enabled {
            push_condition!("enabled = ", enabled);
        }

        if let Some(visibility) = &filters.visibility {
            if !visibility.unscoped {
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                push_sensor_visibility_predicate(&mut qb, visibility);
                push_sensor_visibility_predicate(&mut count_qb, visibility);
            }
        }

        // Suppress unused-assignment warning from the macro's last expansion.
        let _ = has_where;

        // Count
        let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;
        let total = total.max(0) as u64;

        // Data query
        qb.push(" ORDER BY ref ASC");
        qb.push(" LIMIT ");
        qb.push_bind(filters.limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let rows: Vec<Sensor> = qb.build_query_as().fetch_all(db).await?;

        Ok(SensorSearchResult { rows, total })
    }

    /// Find enabled sensors
    pub async fn find_enabled<'e, E>(executor: E) -> Result<Vec<Sensor>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sensors = sqlx::query_as::<_, Sensor>(&format!(
            "SELECT {SENSOR_SELECT_COLUMNS} FROM sensor WHERE enabled = true ORDER BY ref ASC"
        ))
        .fetch_all(executor)
        .await?;

        Ok(sensors)
    }

    /// Find sensors by pack ID
    pub async fn find_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Vec<Sensor>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sensors = sqlx::query_as::<_, Sensor>(&format!(
            "SELECT {SENSOR_SELECT_COLUMNS} FROM sensor WHERE pack = $1 ORDER BY ref ASC"
        ))
        .bind(pack_id)
        .fetch_all(executor)
        .await?;

        Ok(sensors)
    }
}
