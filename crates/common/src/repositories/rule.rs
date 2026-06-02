//! Rule repository for database operations
//!
//! This module provides CRUD operations and queries for Rule entities.

use crate::models::{rule::*, Id};
use crate::{Error, Result};
use sqlx::{Executor, Postgres, QueryBuilder};

use super::{Create, Delete, FindById, FindByRef, List, Patch, Repository, Update};

/// Columns selected when reading `rule` rows. Keep in sync with the `Rule`
/// model struct in `crates/common/src/models.rs`.
pub const SELECT_COLUMNS: &str = "id, ref, pack, pack_ref, label, description, action, action_ref, trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated";

/// Filters for [`RuleRepository::list_search`].
///
/// All fields are optional and combinable (AND). Pagination is always applied.
#[derive(Debug, Clone, Default)]
pub struct RuleSearchFilters {
    /// Filter by pack ID
    pub pack: Option<Id>,
    /// Filter by pack ref
    pub pack_ref: Option<String>,
    /// Filter by action ID
    pub action: Option<Id>,
    /// Filter by action ref
    pub action_ref: Option<String>,
    /// Filter by trigger ID
    pub trigger: Option<Id>,
    /// Filter by trigger ref
    pub trigger_ref: Option<String>,
    /// Filter by enabled status
    pub enabled: Option<bool>,
    pub limit: u32,
    pub offset: u32,
}

/// Result of [`RuleRepository::list_search`].
#[derive(Debug)]
pub struct RuleSearchResult {
    pub rows: Vec<Rule>,
    pub total: u64,
}

/// Input for restoring an ad-hoc rule during pack reinstallation.
/// Unlike `CreateRuleInput`, action and trigger IDs are optional because
/// the referenced entities may not exist yet or may have been removed.
#[derive(Debug, Clone)]
pub struct RestoreRuleInput {
    pub r#ref: String,
    pub pack: Id,
    pub pack_ref: String,
    pub label: String,
    pub description: Option<String>,
    pub action: Option<Id>,
    pub action_ref: String,
    pub trigger: Option<Id>,
    pub trigger_ref: String,
    pub conditions: serde_json::Value,
    pub action_params: serde_json::Value,
    pub trigger_params: serde_json::Value,
    pub permission_set_refs: Option<Vec<String>>,
    pub enabled: bool,
    pub owner_identity: Option<Id>,
}

/// Repository for Rule operations
pub struct RuleRepository;

impl Repository for RuleRepository {
    type Entity = Rule;

    fn table_name() -> &'static str {
        "rules"
    }
}

/// Input for creating a new rule
#[derive(Debug, Clone)]
pub struct CreateRuleInput {
    pub r#ref: String,
    pub pack: Id,
    pub pack_ref: String,
    pub label: String,
    pub description: Option<String>,
    pub action: Id,
    pub action_ref: String,
    pub trigger: Id,
    pub trigger_ref: String,
    pub conditions: serde_json::Value,
    pub action_params: serde_json::Value,
    pub trigger_params: serde_json::Value,
    pub permission_set_refs: Option<Vec<String>>,
    pub enabled: bool,
    pub is_adhoc: bool,
    pub owner_identity: Option<Id>,
}

/// Input for updating a rule
#[derive(Debug, Clone, Default)]
pub struct UpdateRuleInput {
    pub pack: Option<Id>,
    pub pack_ref: Option<String>,
    pub label: Option<String>,
    pub description: Option<Patch<String>>,
    pub action: Option<Id>,
    pub action_ref: Option<String>,
    pub trigger: Option<Id>,
    pub trigger_ref: Option<String>,
    pub conditions: Option<serde_json::Value>,
    pub action_params: Option<serde_json::Value>,
    pub trigger_params: Option<serde_json::Value>,
    pub permission_set_refs: Option<Patch<Vec<String>>>,
    pub enabled: Option<bool>,
    pub is_adhoc: Option<bool>,
    pub owner_identity: Option<Patch<Id>>,
}

#[async_trait::async_trait]
impl FindById for RuleRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rule = sqlx::query_as::<_, Rule>(
            r#"
            SELECT id, ref, pack, pack_ref, label, description, action, action_ref,
                   trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            FROM rule
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(executor)
        .await?;

        Ok(rule)
    }
}

#[async_trait::async_trait]
impl FindByRef for RuleRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rule = sqlx::query_as::<_, Rule>(
            r#"
            SELECT id, ref, pack, pack_ref, label, description, action, action_ref,
                   trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            FROM rule
            WHERE ref = $1
            "#,
        )
        .bind(ref_str)
        .fetch_optional(executor)
        .await?;

        Ok(rule)
    }
}

#[async_trait::async_trait]
impl List for RuleRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rules = sqlx::query_as::<_, Rule>(
            r#"
            SELECT id, ref, pack, pack_ref, label, description, action, action_ref,
                   trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            FROM rule
            ORDER BY ref ASC
            "#,
        )
        .fetch_all(executor)
        .await?;

        Ok(rules)
    }
}

#[async_trait::async_trait]
impl Create for RuleRepository {
    type CreateInput = CreateRuleInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rule = sqlx::query_as::<_, Rule>(
            r#"
            INSERT INTO rule (ref, pack, pack_ref, label, description, action, action_ref,
                              trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            RETURNING id, ref, pack, pack_ref, label, description, action, action_ref,
                      trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            "#,
        )
        .bind(&input.r#ref)
        .bind(input.pack)
        .bind(&input.pack_ref)
        .bind(&input.label)
        .bind(&input.description)
        .bind(input.action)
        .bind(&input.action_ref)
        .bind(input.trigger)
        .bind(&input.trigger_ref)
        .bind(&input.conditions)
        .bind(&input.action_params)
        .bind(&input.trigger_params)
        .bind(&input.permission_set_refs)
        .bind(input.enabled)
        .bind(input.is_adhoc)
        .bind(input.owner_identity)
        .fetch_one(executor)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.is_unique_violation() {
                    return Error::already_exists("Rule", "ref", &input.r#ref);
                }
            }
            e.into()
        })?;

        Ok(rule)
    }
}

#[async_trait::async_trait]
impl Update for RuleRepository {
    type UpdateInput = UpdateRuleInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build update query

        let mut query = QueryBuilder::new("UPDATE rule SET ");
        let mut has_updates = false;

        if let Some(pack) = input.pack {
            query.push("pack = ");
            query.push_bind(pack);
            has_updates = true;
        }

        if let Some(pack_ref) = &input.pack_ref {
            if has_updates {
                query.push(", ");
            }
            query.push("pack_ref = ");
            query.push_bind(pack_ref);
            has_updates = true;
        }

        if let Some(label) = &input.label {
            if has_updates {
                query.push(", ");
            }
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

        if let Some(action) = input.action {
            if has_updates {
                query.push(", ");
            }
            query.push("action = ");
            query.push_bind(action);
            has_updates = true;
        }

        if let Some(action_ref) = &input.action_ref {
            if has_updates {
                query.push(", ");
            }
            query.push("action_ref = ");
            query.push_bind(action_ref);
            has_updates = true;
        }

        if let Some(trigger) = input.trigger {
            if has_updates {
                query.push(", ");
            }
            query.push("trigger = ");
            query.push_bind(trigger);
            has_updates = true;
        }

        if let Some(trigger_ref) = &input.trigger_ref {
            if has_updates {
                query.push(", ");
            }
            query.push("trigger_ref = ");
            query.push_bind(trigger_ref);
            has_updates = true;
        }

        if let Some(conditions) = &input.conditions {
            if has_updates {
                query.push(", ");
            }
            query.push("conditions = ");
            query.push_bind(conditions);
            has_updates = true;
        }

        if let Some(action_params) = &input.action_params {
            if has_updates {
                query.push(", ");
            }
            query.push("action_params = ");
            query.push_bind(action_params);
            has_updates = true;
        }

        if let Some(trigger_params) = &input.trigger_params {
            if has_updates {
                query.push(", ");
            }
            query.push("trigger_params = ");
            query.push_bind(trigger_params);
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

        if let Some(enabled) = input.enabled {
            if has_updates {
                query.push(", ");
            }
            query.push("enabled = ");
            query.push_bind(enabled);
            has_updates = true;
        }

        if let Some(is_adhoc) = input.is_adhoc {
            if has_updates {
                query.push(", ");
            }
            query.push("is_adhoc = ");
            query.push_bind(is_adhoc);
            has_updates = true;
        }

        if let Some(owner_identity) = &input.owner_identity {
            if has_updates {
                query.push(", ");
            }
            query.push("owner_identity = ");
            match owner_identity {
                Patch::Set(value) => query.push_bind(Some(*value)),
                Patch::Clear => query.push_bind(Option::<Id>::None),
            };
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ");
        query.push_bind(id);
        query.push(" RETURNING id, ref, pack, pack_ref, label, description, action, action_ref, trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated");

        let rule = query.build_query_as::<Rule>().fetch_one(executor).await?;

        Ok(rule)
    }
}

#[async_trait::async_trait]
impl Delete for RuleRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM rule WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

impl RuleRepository {
    /// Search rules with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    pub async fn list_search<'e, E>(db: E, filters: &RuleSearchFilters) -> Result<RuleSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let select_cols = SELECT_COLUMNS;

        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {select_cols} FROM rule"));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM rule");

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
        if let Some(ref pack_ref) = filters.pack_ref {
            push_condition!("pack_ref = ", pack_ref);
        }
        if let Some(action_id) = filters.action {
            push_condition!("action = ", action_id);
        }
        if let Some(ref action_ref) = filters.action_ref {
            push_condition!("action_ref = ", action_ref);
        }
        if let Some(trigger_id) = filters.trigger {
            push_condition!("trigger = ", trigger_id);
        }
        if let Some(ref trigger_ref) = filters.trigger_ref {
            push_condition!("trigger_ref = ", trigger_ref);
        }
        if let Some(enabled) = filters.enabled {
            push_condition!("enabled = ", enabled);
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

        let rows: Vec<Rule> = qb.build_query_as().fetch_all(db).await?;

        Ok(RuleSearchResult { rows, total })
    }

    /// Find rules by pack ID
    pub async fn find_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Vec<Rule>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rules = sqlx::query_as::<_, Rule>(
            r#"
            SELECT id, ref, pack, pack_ref, label, description, action, action_ref,
                   trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            FROM rule
            WHERE pack = $1
            ORDER BY ref ASC
            "#,
        )
        .bind(pack_id)
        .fetch_all(executor)
        .await?;

        Ok(rules)
    }

    /// Find rules by action ID
    pub async fn find_by_action<'e, E>(executor: E, action_id: Id) -> Result<Vec<Rule>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rules = sqlx::query_as::<_, Rule>(
            r#"
            SELECT id, ref, pack, pack_ref, label, description, action, action_ref,
                   trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            FROM rule
            WHERE action = $1
            ORDER BY ref ASC
            "#,
        )
        .bind(action_id)
        .fetch_all(executor)
        .await?;

        Ok(rules)
    }

    /// Find rules by trigger ID
    pub async fn find_by_trigger<'e, E>(executor: E, trigger_id: Id) -> Result<Vec<Rule>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rules = sqlx::query_as::<_, Rule>(
            r#"
            SELECT id, ref, pack, pack_ref, label, description, action, action_ref,
                   trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            FROM rule
            WHERE trigger = $1
            ORDER BY ref ASC
            "#,
        )
        .bind(trigger_id)
        .fetch_all(executor)
        .await?;

        Ok(rules)
    }

    /// Find enabled rules
    pub async fn find_enabled<'e, E>(executor: E) -> Result<Vec<Rule>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rules = sqlx::query_as::<_, Rule>(
            r#"
            SELECT id, ref, pack, pack_ref, label, description, action, action_ref,
                   trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            FROM rule
            WHERE enabled = true
            ORDER BY ref ASC
            "#,
        )
        .fetch_all(executor)
        .await?;

        Ok(rules)
    }

    /// Find ad-hoc (user-created) rules belonging to a specific pack.
    /// Used to preserve custom rules during pack reinstallation.
    pub async fn find_adhoc_by_pack<'e, E>(executor: E, pack_id: Id) -> Result<Vec<Rule>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rules = sqlx::query_as::<_, Rule>(
            r#"
            SELECT id, ref, pack, pack_ref, label, description, action, action_ref,
                   trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            FROM rule
            WHERE pack = $1 AND is_adhoc = true
            ORDER BY ref ASC
            "#,
        )
        .bind(pack_id)
        .fetch_all(executor)
        .await?;

        Ok(rules)
    }

    /// Restore an ad-hoc rule after pack reinstallation.
    /// Accepts `Option<Id>` for action and trigger so the rule is preserved
    /// even if its referenced entities no longer exist.
    pub async fn restore_rule<'e, E>(executor: E, input: RestoreRuleInput) -> Result<Rule>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rule = sqlx::query_as::<_, Rule>(
            r#"
            INSERT INTO rule (ref, pack, pack_ref, label, description, action, action_ref,
                              trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, true, $15)
            RETURNING id, ref, pack, pack_ref, label, description, action, action_ref,
                      trigger, trigger_ref, conditions, action_params, trigger_params, permission_set_refs, enabled, is_adhoc, owner_identity, created, updated
            "#,
        )
        .bind(&input.r#ref)
        .bind(input.pack)
        .bind(&input.pack_ref)
        .bind(&input.label)
        .bind(&input.description)
        .bind(input.action)
        .bind(&input.action_ref)
        .bind(input.trigger)
        .bind(&input.trigger_ref)
        .bind(&input.conditions)
        .bind(&input.action_params)
        .bind(&input.trigger_params)
        .bind(&input.permission_set_refs)
        .bind(input.enabled)
        .bind(input.owner_identity)
        .fetch_one(executor)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.is_unique_violation() {
                    return Error::already_exists("Rule", "ref", &input.r#ref);
                }
            }
            e.into()
        })?;

        Ok(rule)
    }

    /// Re-link rules whose action FK is NULL back to a newly recreated action,
    /// matched by `action_ref`. Used after pack reinstallation to fix rules
    /// from other packs that referenced actions in the reinstalled pack.
    pub async fn relink_action_by_ref<'e, E>(
        executor: E,
        action_ref: &str,
        action_id: Id,
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            r#"
            UPDATE rule
            SET action = $1, updated = NOW()
            WHERE action IS NULL AND action_ref = $2
            "#,
        )
        .bind(action_id)
        .bind(action_ref)
        .execute(executor)
        .await?;

        Ok(result.rows_affected())
    }

    /// Re-link rules whose trigger FK is NULL back to a newly recreated trigger,
    /// matched by `trigger_ref`. Used after pack reinstallation to fix rules
    /// from other packs that referenced triggers in the reinstalled pack.
    pub async fn relink_trigger_by_ref<'e, E>(
        executor: E,
        trigger_ref: &str,
        trigger_id: Id,
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            r#"
            UPDATE rule
            SET trigger = $1, updated = NOW()
            WHERE trigger IS NULL AND trigger_ref = $2
            "#,
        )
        .bind(trigger_id)
        .bind(trigger_ref)
        .execute(executor)
        .await?;

        Ok(result.rows_affected())
    }

    /// Delete pack-owned (non-ad-hoc) rules for a pack, excluding the supplied refs.
    ///
    /// Used by pack reload to remove declarative rules that were deleted from
    /// `rules/*.yaml` while preserving API/UI-created ad-hoc rules.
    pub async fn delete_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        refs: &[String],
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = if refs.is_empty() {
            sqlx::query("DELETE FROM rule WHERE pack = $1 AND is_adhoc = false")
                .bind(pack_id)
                .execute(executor)
                .await?
        } else {
            sqlx::query("DELETE FROM rule WHERE pack = $1 AND is_adhoc = false AND ref != ALL($2)")
                .bind(pack_id)
                .bind(refs)
                .execute(executor)
                .await?
        };

        Ok(result.rows_affected())
    }
}
