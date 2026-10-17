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
    pub pinned_targets: Option<Vec<crate::runtime_authority::BoundRuntimeTarget>>,
    pub pinned_owners: Option<Vec<(String, Id, Option<Id>)>>,
    pub pinned_packs: Option<Vec<Id>>,
    pub ownership: KeyOwnershipScope,
    pub pack_refs: Option<Vec<String>>,
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

#[derive(Debug, Clone, Copy, Default)]
pub enum KeyOwnershipScope {
    #[default]
    Any,
    ManagedPack(Id),
    Pack(Id),
    Action(Id),
}

impl KeyGrantFilter {
    /// Compile recorded constraints without erasing their managing-pack envelope.
    /// Human Key authorization does not populate pack context and keeps its
    /// separate compiler until the canonical IAM conversion is complete.
    pub fn from_recorded_grant(
        grant: &crate::rbac::Grant,
        ownership: KeyOwnershipScope,
    ) -> Option<Self> {
        use crate::rbac::{Action, ExecutionScopeConstraint, Resource};
        if grant.resource != Resource::Keys || !grant.actions.contains(&Action::Read) {
            return None;
        }
        let Some(scope) = &grant.constraints else {
            return Some(Self {
                ownership,
                ..Default::default()
            });
        };
        let ownership = if let Some(pack) = scope.managing_pack_id {
            match ownership {
                KeyOwnershipScope::Any => KeyOwnershipScope::ManagedPack(pack),
                KeyOwnershipScope::ManagedPack(existing) if existing == pack => ownership,
                _ => return None,
            }
        } else {
            ownership
        };
        if scope.visibility.is_some()
            || matches!(
                scope.execution_scope,
                Some(ExecutionScopeConstraint::SelfOnly | ExecutionScopeConstraint::Descendants)
            )
            || scope
                .attributes
                .as_ref()
                .is_some_and(|attributes| !attributes.is_empty())
        {
            return None;
        }
        Some(Self {
            pinned_targets: None,
            pinned_owners: None,
            pinned_packs: None,
            ownership,
            pack_refs: scope.pack_refs.clone(),
            owner_types: scope.owner_types.clone(),
            owner: scope.owner,
            owner_refs: scope.owner_refs.clone(),
            refs: scope.refs.clone(),
            ids: scope.ids.clone(),
            encrypted: scope.encrypted,
            owner_scoped: scope.owner.is_some()
                || scope.owner_types.is_some()
                || scope.owner_refs.is_some()
                || scope.refs.is_some()
                || scope.ids.is_some(),
        })
    }
}

/// Result of [`KeyRepository::search`].
#[derive(Debug)]
pub struct KeySearchResult {
    pub rows: Vec<Key>,
    pub total: u64,
}

pub struct KeyRepository;

pub const SELECT_COLUMNS: &str = "id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated";

impl KeyRepository {
    pub async fn find_by_ref_for_share(
        connection: &mut sqlx::PgConnection,
        key_ref: &str,
    ) -> Result<Option<Key>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM key WHERE ref = $1 FOR SHARE"
        ))
        .bind(key_ref)
        .fetch_optional(connection)
        .await?)
    }
    pub async fn find_by_ref_for_update(
        connection: &mut sqlx::PgConnection,
        key_ref: &str,
    ) -> Result<Option<Key>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM key WHERE ref = $1 FOR UPDATE"
        ))
        .bind(key_ref)
        .fetch_optional(connection)
        .await?)
    }
    pub async fn find_by_refs<'e, E>(executor: E, refs: &[String]) -> Result<Vec<Key>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if refs.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM key WHERE ref = ANY($1) ORDER BY ref"
        ))
        .bind(refs)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }
}

impl Repository for KeyRepository {
    type Entity = Key;
    fn table_name() -> &'static str {
        "key"
    }
}

#[derive(Debug, Clone)]
pub struct CreateKeyInput {
    pub local_ref: String,
    pub owner_type: OwnerType,
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
            "SELECT id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated FROM key WHERE id = $1"
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
            "SELECT id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated FROM key ORDER BY ref ASC"
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
            "INSERT INTO key (local_ref, owner_type, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated"
        ).bind(&input.local_ref).bind(input.owner_type).bind(input.owner_identity).bind(input.owner_pack).bind(&input.owner_pack_ref).bind(input.owner_action).bind(&input.owner_action_ref).bind(input.owner_sensor).bind(&input.owner_sensor_ref).bind(&input.name).bind(input.encrypted).bind(&input.encryption_key_hash).bind(&input.value).fetch_one(executor).await.map_err(Into::into)
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
        if input.encrypted == Some(false) {
            if has_updates {
                query.push(", ");
            }
            query.push("encryption_key_hash = NULL");
            has_updates = true;
        } else if let Some(encryption_key_hash) = &input.encryption_key_hash {
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
        query.push(" RETURNING id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated");

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
            "SELECT id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated FROM key WHERE ref = $1"
        ).bind(ref_str).fetch_optional(executor).await.map_err(Into::into)
    }

    pub async fn find_by_owner_type<'e, E>(executor: E, owner_type: OwnerType) -> Result<Vec<Key>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Key>(
            "SELECT id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated FROM key WHERE owner_type = $1 ORDER BY ref ASC"
        ).bind(owner_type).fetch_all(executor).await.map_err(Into::into)
    }

    /// Search keys with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    pub async fn search<'e, E>(db: E, filters: &KeySearchFilters) -> Result<KeySearchResult>
    where
        E: sqlx::Acquire<'e, Database = Postgres> + Send + 'e,
    {
        let mut connection = db.acquire().await?;
        Self::search_with_connection(&mut connection, filters).await
    }

    pub async fn search_with_connection(
        connection: &mut sqlx::PgConnection,
        filters: &KeySearchFilters,
    ) -> Result<KeySearchResult> {
        let select_cols = "id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted, encryption_key_hash, value, created, updated";

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
        let total: i64 = count_qb
            .build_query_scalar()
            .fetch_one(&mut *connection)
            .await?;
        let total = total.max(0) as u64;

        // Data query
        qb.push(" ORDER BY ref ASC");
        qb.push(" LIMIT ");
        qb.push_bind(filters.limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let rows: Vec<Key> = qb.build_query_as().fetch_all(&mut *connection).await?;

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

    match grant.ownership {
        KeyOwnershipScope::Any => {}
        KeyOwnershipScope::ManagedPack(pack) => {
            and_sep!();
            qb.push(KEY_MANAGING_PACK_ID_SQL)
                .push(" = ")
                .push_bind(pack);
        }
        KeyOwnershipScope::Pack(pack) => {
            and_sep!();
            qb.push("(key.owner_type = 'pack' AND key.owner_pack = ")
                .push_bind(pack)
                .push(")");
        }
        KeyOwnershipScope::Action(action) => {
            and_sep!();
            qb.push("(key.owner_type = 'action' AND key.owner_action = ")
                .push_bind(action)
                .push(")");
        }
    }
    if let Some(owners) = &grant.pinned_owners {
        and_sep!();
        super::runtime_target_binding::push_owner_clause(
            qb,
            "key",
            KEY_MANAGING_PACK_ID_SQL,
            owners,
        );
    }
    if let Some(targets) = &grant.pinned_targets {
        and_sep!();
        super::runtime_target_binding::push_target_clause(
            qb,
            "key",
            KEY_MANAGING_PACK_ID_SQL,
            targets,
        );
    }
    if let Some(packs) = &grant.pinned_packs {
        and_sep!();
        qb.push(KEY_MANAGING_PACK_ID_SQL)
            .push(" = ANY(")
            .push_bind(packs.clone())
            .push(")");
    }
    if let Some(pack_refs) = &grant.pack_refs {
        and_sep!();
        qb.push("EXISTS (SELECT 1 FROM pack scoped_pack WHERE scoped_pack.id = ")
            .push(KEY_MANAGING_PACK_ID_SQL)
            .push(" AND scoped_pack.ref = ANY(")
            .push_bind(pack_refs.clone())
            .push("))");
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

const KEY_MANAGING_PACK_ID_SQL: &str = "(CASE key.owner_type \
    WHEN 'pack' THEN (SELECT p.id FROM pack p WHERE p.id = key.owner_pack) \
    WHEN 'action' THEN (SELECT a.pack FROM action a WHERE a.id = key.owner_action AND a.management_origin = 'pack') \
    WHEN 'sensor' THEN (SELECT s.pack FROM sensor s WHERE s.id = key.owner_sensor AND s.management_origin = 'pack') \
    ELSE NULL END)";

#[cfg(test)]
mod recorded_filter_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recorded_filters_retain_owner_envelopes_and_narrowing_constraints() {
        let grant = serde_json::from_value(json!({"resource":"keys","actions":["read"],
            "constraints":{"pack_refs":["local"],"encrypted":true}}))
        .unwrap();
        let filter =
            KeyGrantFilter::from_recorded_grant(&grant, KeyOwnershipScope::ManagedPack(42))
                .unwrap();
        assert!(matches!(
            filter.ownership,
            KeyOwnershipScope::ManagedPack(42)
        ));
        assert_eq!(filter.pack_refs, Some(vec!["local".into()]));
        assert_eq!(filter.encrypted, Some(true));
        let mut query = QueryBuilder::<Postgres>::new("SELECT id FROM key WHERE ");
        push_grant_clause(&mut query, 17, &filter);
        let sql = query.sql();
        assert!(sql.contains("a.management_origin = 'pack'"));
        assert!(sql.contains("s.management_origin = 'pack'"));
        assert!(sql.contains("scoped_pack.ref = ANY("));
        assert!(!sql.contains("LIKE"));
    }

    #[test]
    fn recorded_filters_reject_unpopulated_context_and_non_read_capabilities() {
        for constraints in [
            json!({"visibility":["public"]}),
            json!({"execution_scope":"self"}),
            json!({"attributes":{"team":"ops"}}),
        ] {
            let grant = serde_json::from_value(
                json!({"resource":"keys","actions":["read"],"constraints":constraints}),
            )
            .unwrap();
            assert!(KeyGrantFilter::from_recorded_grant(&grant, KeyOwnershipScope::Any).is_none());
        }
        for value in [
            json!({"resource":"keys","actions":["use"]}),
            json!({"resource":"events","actions":["read"]}),
        ] {
            let grant = serde_json::from_value(value).unwrap();
            assert!(KeyGrantFilter::from_recorded_grant(&grant, KeyOwnershipScope::Any).is_none());
        }
    }
}
