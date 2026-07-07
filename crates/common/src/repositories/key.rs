//! Key/Secret repository for database operations

use crate::models::{key::*, Id, OwnerType};
use crate::rbac::OwnerConstraint;
use crate::Result;
use serde_json::Value as JsonValue;
use sqlx::{Executor, Postgres, QueryBuilder};

use super::{Create, Delete, FindById, List, Repository, Update};

/// Filters for [`KeyRepository::search`].
///
/// All fields are optional and combinable (AND). Pagination is always applied.
#[derive(Debug, Clone, Default)]
pub struct KeySearchFilters {
    pub owner_type: Option<OwnerType>,
    pub owner: Option<String>,
    pub limit: u32,
    pub offset: u32,
    /// Row-level RBAC visibility to apply in SQL. When `Some`, only rows
    /// satisfying at least one of the compiled grant filters are returned
    /// (fail-closed: `None` grants means every row is excluded; an empty
    /// `grants` list means no key is visible).
    pub visibility: Option<KeyVisibility>,
}

/// A single caller's key-read visibility, compiled from their effective RBAC
/// grants (see `crates/api/src/routes/keys.rs::compile_key_read_grant_filters`).
///
/// Rows are visible if they match at least one [`KeyGrantFilter`] in `grants`.
#[derive(Debug, Clone)]
pub struct KeyVisibility {
    pub identity_id: Id,
    pub grants: Vec<KeyGrantFilter>,
}

/// SQL-translatable subset of a single [`crate::rbac::Grant`]'s constraints,
/// restricted to the fields that are meaningful for `key` row visibility.
///
/// Grants whose constraints can never be satisfied for keys (e.g. `pack_refs`,
/// `visibility`, non-`Any` `execution_scope`, or non-empty `attributes` — none
/// of which the key `AuthorizationContext` ever populates) must be excluded
/// by the caller before reaching SQL; they are not represented here.
#[derive(Debug, Clone, Default)]
pub struct KeyGrantFilter {
    pub owner_types: Option<Vec<OwnerType>>,
    pub owner: Option<OwnerConstraint>,
    pub owner_refs: Option<Vec<String>>,
    pub refs: Option<Vec<String>>,
    pub ids: Option<Vec<Id>>,
    pub encrypted: Option<bool>,
    /// Mirrors `constrained_key_grant_allows`'s "owner scoped" test: true
    /// when this grant carries at least one owner/ref/id constraint. Grants
    /// that are not owner-scoped never grant visibility into another
    /// identity's `owner_type = 'identity'` keys.
    pub owner_scoped: bool,
}

/// Result of [`KeyRepository::search`].
#[derive(Debug)]
pub struct KeySearchResult {
    pub rows: Vec<Key>,
    pub total: u64,
}

pub struct KeyRepository;

impl Repository for KeyRepository {
    type Entity = Key;
    fn table_name() -> &'static str {
        "key"
    }
}

#[derive(Debug, Clone)]
pub struct CreateKeyInput {
    pub r#ref: String,
    pub owner_type: OwnerType,
    pub owner: Option<String>,
    pub owner_identity: Option<Id>,
    pub owner_pack: Option<Id>,
    pub owner_pack_ref: Option<String>,
    pub owner_action: Option<Id>,
    pub owner_action_ref: Option<String>,
    pub owner_sensor: Option<Id>,
    pub owner_sensor_ref: Option<String>,
    pub name: String,
    pub encrypted: bool,
    pub encryption_key_hash: Option<String>,
    pub value: JsonValue,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateKeyInput {
    pub name: Option<String>,
    pub value: Option<JsonValue>,
    pub encrypted: Option<bool>,
    pub encryption_key_hash: Option<String>,
}

#[async_trait::async_trait]
impl FindById for KeyRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Key>(
            "SELECT id, ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated FROM key WHERE id = $1"
        ).bind(id).fetch_optional(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for KeyRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Key>(
            "SELECT id, ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated FROM key ORDER BY ref ASC"
        ).fetch_all(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for KeyRepository {
    type CreateInput = CreateKeyInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Key>(
            "INSERT INTO key (ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING id, ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated"
        ).bind(&input.r#ref).bind(input.owner_type).bind(&input.owner).bind(input.owner_identity).bind(input.owner_pack).bind(&input.owner_pack_ref).bind(input.owner_action).bind(&input.owner_action_ref).bind(input.owner_sensor).bind(&input.owner_sensor_ref).bind(&input.name).bind(input.encrypted).bind(&input.encryption_key_hash).bind(&input.value).fetch_one(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for KeyRepository {
    type UpdateInput = UpdateKeyInput;
    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build update query
        let mut query = QueryBuilder::new("UPDATE key SET ");
        let mut has_updates = false;

        if let Some(name) = &input.name {
            query.push("name = ").push_bind(name);
            has_updates = true;
        }
        if let Some(value) = &input.value {
            if has_updates {
                query.push(", ");
            }
            query.push("value = ").push_bind(value);
            has_updates = true;
        }
        if let Some(encrypted) = input.encrypted {
            if has_updates {
                query.push(", ");
            }
            query.push("encrypted = ").push_bind(encrypted);
            has_updates = true;
        }
        if let Some(encryption_key_hash) = &input.encryption_key_hash {
            if has_updates {
                query.push(", ");
            }
            query
                .push("encryption_key_hash = ")
                .push_bind(encryption_key_hash);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(" RETURNING id, ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated");

        query
            .build_query_as::<Key>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for KeyRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM key WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl KeyRepository {
    pub async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Key>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Key>(
            "SELECT id, ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated FROM key WHERE ref = $1"
        ).bind(ref_str).fetch_optional(executor).await.map_err(Into::into)
    }

    pub async fn find_by_owner_type<'e, E>(executor: E, owner_type: OwnerType) -> Result<Vec<Key>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Key>(
            "SELECT id, ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated FROM key WHERE owner_type = $1 ORDER BY ref ASC"
        ).bind(owner_type).fetch_all(executor).await.map_err(Into::into)
    }

    /// Search keys with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    pub async fn search<'e, E>(db: E, filters: &KeySearchFilters) -> Result<KeySearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let select_cols = "id, ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated";

        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {select_cols} FROM key"));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM key");

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

        if let Some(ref owner_type) = filters.owner_type {
            push_condition!("owner_type = ", *owner_type);
        }
        if let Some(ref owner) = filters.owner {
            push_condition!("owner = ", owner.clone());
        }

        if let Some(visibility) = &filters.visibility {
            if !has_where {
                qb.push(" WHERE ");
                count_qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
                count_qb.push(" AND ");
            }
            push_visibility_clause(&mut qb, visibility);
            push_visibility_clause(&mut count_qb, visibility);
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

        let rows: Vec<Key> = qb.build_query_as().fetch_all(db).await?;

        Ok(KeySearchResult { rows, total })
    }
}

/// Appends `(grant_1_clause OR grant_2_clause OR ...)` to `qb`, or `FALSE`
/// when there are no usable grants (fail-closed: nothing is visible).
fn push_visibility_clause(qb: &mut QueryBuilder<'_, Postgres>, visibility: &KeyVisibility) {
    if visibility.grants.is_empty() {
        qb.push("FALSE");
        return;
    }

    qb.push("(");
    for (i, grant) in visibility.grants.iter().enumerate() {
        if i > 0 {
            qb.push(" OR ");
        }
        push_grant_clause(qb, visibility.identity_id, grant);
    }
    qb.push(")");
}

/// Translates a single [`KeyGrantFilter`] into a SQL boolean expression that
/// mirrors `Grant::constraints_match` + `constrained_key_grant_allows` from
/// `crates/api/src/routes/keys.rs`, operating on the current `key` row.
fn push_grant_clause(qb: &mut QueryBuilder<'_, Postgres>, identity_id: Id, grant: &KeyGrantFilter) {
    qb.push("(");
    let mut first = true;

    macro_rules! and_sep {
        () => {
            if first {
                first = false;
            } else {
                qb.push(" AND ");
            }
        };
    }

    if let Some(owner_types) = &grant.owner_types {
        and_sep!();
        if owner_types.is_empty() {
            qb.push("FALSE");
        } else {
            qb.push("owner_type IN (");
            {
                let mut sep = qb.separated(", ");
                for owner_type in owner_types {
                    sep.push_bind(*owner_type);
                }
            }
            qb.push(")");
        }
    }

    if let Some(owner) = grant.owner {
        and_sep!();
        match owner {
            OwnerConstraint::SelfOnly => {
                qb.push("owner_identity = ");
                qb.push_bind(identity_id);
            }
            OwnerConstraint::Any => {
                qb.push("TRUE");
            }
            OwnerConstraint::None => {
                qb.push("owner_identity IS NULL");
            }
        }
    }

    if let Some(owner_refs) = &grant.owner_refs {
        and_sep!();
        if owner_refs.is_empty() {
            qb.push("FALSE");
        } else {
            // Mirrors `key_owner_ref`: the effective owner ref column
            // depends on owner_type.
            qb.push("(CASE owner_type WHEN ");
            qb.push_bind(OwnerType::Pack);
            qb.push(" THEN owner_pack_ref WHEN ");
            qb.push_bind(OwnerType::Action);
            qb.push(" THEN owner_action_ref WHEN ");
            qb.push_bind(OwnerType::Sensor);
            qb.push(" THEN owner_sensor_ref ELSE owner END) IN (");
            {
                let mut sep = qb.separated(", ");
                for owner_ref in owner_refs {
                    sep.push_bind(owner_ref.clone());
                }
            }
            qb.push(")");
        }
    }

    if let Some(refs) = &grant.refs {
        and_sep!();
        if refs.is_empty() {
            qb.push("FALSE");
        } else {
            qb.push("ref IN (");
            {
                let mut sep = qb.separated(", ");
                for r in refs {
                    sep.push_bind(r.clone());
                }
            }
            qb.push(")");
        }
    }

    if let Some(ids) = &grant.ids {
        and_sep!();
        if ids.is_empty() {
            qb.push("FALSE");
        } else {
            qb.push("id IN (");
            {
                let mut sep = qb.separated(", ");
                for id in ids {
                    sep.push_bind(*id);
                }
            }
            qb.push(")");
        }
    }

    if let Some(encrypted) = grant.encrypted {
        and_sep!();
        qb.push("encrypted = ");
        qb.push_bind(encrypted);
    }

    if !grant.owner_scoped {
        // Fail-closed: a grant without owner/ref/id scoping must not expose
        // another identity's `owner_type = 'identity'` keys, matching
        // `key_action_allowed`'s special case for identity-owned keys.
        and_sep!();
        qb.push("NOT (owner_type = ");
        qb.push_bind(OwnerType::Identity);
        qb.push(" AND owner_identity IS DISTINCT FROM ");
        qb.push_bind(identity_id);
        qb.push(")");
    }

    if first {
        qb.push("TRUE");
    }

    qb.push(")");
}
