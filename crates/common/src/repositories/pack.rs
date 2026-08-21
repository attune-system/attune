//! Pack repository for database operations on packs
//!
//! This module provides CRUD operations and queries for Pack entities.

use crate::models::{pack::Pack, Id, JsonDict, JsonSchema};
use crate::rbac::OwnerConstraint;
use crate::schema::RefValidator;
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use sqlx::{Executor, Postgres, QueryBuilder};

use super::{
    text_search_patterns, Create, Delete, FindById, FindByRef, List, Pagination, Patch, Repository,
    Update,
};

/// Repository for Pack operations
pub struct PackRepository;

impl Repository for PackRepository {
    type Entity = Pack;

    fn table_name() -> &'static str {
        "pack"
    }
}

/// Input for creating a new pack
#[derive(Debug, Clone)]
pub struct CreatePackInput {
    pub r#ref: String,
    pub label: String,
    pub description: Option<String>,
    pub version: String,
    pub conf_schema: JsonSchema,
    pub config: JsonDict,
    pub meta: JsonDict,
    pub tags: Vec<String>,
    pub runtime_deps: Vec<String>,
    pub dependencies: Vec<String>,
    pub is_standard: bool,
    pub installers: JsonDict,
}

/// Input for updating a pack
#[derive(Debug, Clone, Default)]
pub struct UpdatePackInput {
    pub label: Option<String>,
    pub description: Option<Patch<String>>,
    pub version: Option<String>,
    pub conf_schema: Option<JsonSchema>,
    pub config: Option<JsonDict>,
    pub meta: Option<JsonDict>,
    pub tags: Option<Vec<String>>,
    pub runtime_deps: Option<Vec<String>>,
    pub dependencies: Option<Vec<String>>,
    pub is_standard: Option<bool>,
    pub installers: Option<JsonDict>,
}

const PACK_COLUMNS: &str = "id, ref, label, description, version, conf_schema, config, meta, tags, runtime_deps, dependencies, is_standard, installers, worker_selector, worker_tolerations, worker_affinity, source_type, source_url, source_ref, checksum, checksum_verified, installed_at, installed_by, installation_method, storage_path, install_status, created, updated";

#[async_trait::async_trait]
impl FindById for PackRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM pack WHERE id = $1", PACK_COLUMNS);
        let pack = sqlx::query_as::<_, Pack>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await?;

        Ok(pack)
    }
}

#[async_trait::async_trait]
impl FindByRef for PackRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM pack WHERE ref = $1", PACK_COLUMNS);
        let pack = sqlx::query_as::<_, Pack>(&query)
            .bind(ref_str)
            .fetch_optional(executor)
            .await?;

        Ok(pack)
    }
}

#[async_trait::async_trait]
impl List for PackRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM pack ORDER BY ref ASC", PACK_COLUMNS);
        let packs = sqlx::query_as::<_, Pack>(&query)
            .fetch_all(executor)
            .await?;

        Ok(packs)
    }
}

#[async_trait::async_trait]
impl Create for PackRepository {
    type CreateInput = CreatePackInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        RefValidator::validate_pack_ref(&input.r#ref)?;

        let query = format!(
            r#"
            INSERT INTO pack (ref, label, description, version, conf_schema, config, meta,
                              tags, runtime_deps, dependencies, is_standard, installers)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING {}
            "#,
            PACK_COLUMNS
        );

        // Try to insert - database will enforce uniqueness constraint
        let pack = sqlx::query_as::<_, Pack>(&query)
            .bind(&input.r#ref)
            .bind(&input.label)
            .bind(&input.description)
            .bind(&input.version)
            .bind(&input.conf_schema)
            .bind(&input.config)
            .bind(&input.meta)
            .bind(&input.tags)
            .bind(&input.runtime_deps)
            .bind(&input.dependencies)
            .bind(input.is_standard)
            .bind(&input.installers)
            .fetch_one(executor)
            .await
            .map_err(|e| {
                // Convert unique constraint violation to AlreadyExists error
                if let sqlx::Error::Database(db_err) = &e {
                    if db_err.is_unique_violation() {
                        return Error::already_exists("Pack", "ref", &input.r#ref);
                    }
                }
                e.into()
            })?;

        Ok(pack)
    }
}

#[async_trait::async_trait]
impl Update for PackRepository {
    type UpdateInput = UpdatePackInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build dynamic UPDATE query
        let mut query = QueryBuilder::new("UPDATE pack SET ");
        let mut has_updates = false;

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

        if let Some(version) = &input.version {
            if has_updates {
                query.push(", ");
            }
            query.push("version = ");
            query.push_bind(version);
            has_updates = true;
        }

        if let Some(conf_schema) = &input.conf_schema {
            if has_updates {
                query.push(", ");
            }
            query.push("conf_schema = ");
            query.push_bind(conf_schema);
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

        if let Some(meta) = &input.meta {
            if has_updates {
                query.push(", ");
            }
            query.push("meta = ");
            query.push_bind(meta);
            has_updates = true;
        }

        if let Some(tags) = &input.tags {
            if has_updates {
                query.push(", ");
            }
            query.push("tags = ");
            query.push_bind(tags);
            has_updates = true;
        }

        if let Some(runtime_deps) = &input.runtime_deps {
            if has_updates {
                query.push(", ");
            }
            query.push("runtime_deps = ");
            query.push_bind(runtime_deps);
            has_updates = true;
        }

        if let Some(dependencies) = &input.dependencies {
            if has_updates {
                query.push(", ");
            }
            query.push("dependencies = ");
            query.push_bind(dependencies);
            has_updates = true;
        }

        if let Some(is_standard) = input.is_standard {
            if has_updates {
                query.push(", ");
            }
            query.push("is_standard = ");
            query.push_bind(is_standard);
            has_updates = true;
        }

        if let Some(installers) = &input.installers {
            if has_updates {
                query.push(", ");
            }
            query.push("installers = ");
            query.push_bind(installers);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing pack
            return Self::find_by_id(executor, id)
                .await?
                .ok_or_else(|| Error::not_found("pack", "id", id.to_string()));
        }

        // Add updated timestamp
        query.push(", updated = NOW() WHERE id = ");
        query.push_bind(id);
        query.push(" RETURNING ");
        query.push(PACK_COLUMNS);

        let pack = query
            .build_query_as::<Pack>()
            .fetch_one(executor)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => Error::not_found("pack", "id", id.to_string()),
                _ => e.into(),
            })?;

        Ok(pack)
    }
}

#[async_trait::async_trait]
impl Delete for PackRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM pack WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

/// One OR-branch of pack visibility, derived from a single RBAC grant.
///
/// Each `Some` field is an AND-ed condition; a scope with all fields `None`
/// matches every row (i.e. an unconstrained grant).
#[derive(Debug, Clone, Default)]
pub struct PackVisibilityScope {
    /// Constrains by `pack.installed_by` relative to the requesting identity.
    pub owner: Option<OwnerConstraint>,
    /// Allowed pack refs (matches `pack.ref`, from the grant's `pack_refs`).
    pub pack_refs: Option<Vec<String>>,
    /// Allowed pack refs (matches `pack.ref`, from the grant's `refs`).
    pub refs: Option<Vec<String>>,
    /// Allowed pack IDs (matches `pack.id`).
    pub ids: Option<Vec<Id>>,
}

/// SQL-evaluable RBAC visibility filter for [`PackRepository::list_search`].
///
/// Mirrors the row-level semantics of the API's `pack_action_allowed`
/// helper: standard packs are always visible; scopes in
/// `own_or_ownerless_scopes` apply to packs the identity installed or that
/// have no owner; scopes in `other_owner_scopes` are the subset of grants
/// specific enough to also see packs installed by someone else.
#[derive(Debug, Clone, Default)]
pub struct PackVisibilityFilter {
    pub identity_id: Id,
    pub own_or_ownerless_scopes: Vec<PackVisibilityScope>,
    pub other_owner_scopes: Vec<PackVisibilityScope>,
}

/// Filters for [`PackRepository::list_search`].
#[derive(Debug, Clone, Default)]
pub struct PackSearchFilters {
    /// `None` applies no RBAC restriction. `Some` (even with both scope
    /// lists empty) restricts to standard packs plus whatever the scopes
    /// allow.
    pub visibility: Option<PackVisibilityFilter>,
    /// Text search across ref, label, and description.
    pub query: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone)]
pub struct PackSearchResult {
    pub rows: Vec<Pack>,
    pub total: u64,
}

fn push_pack_scope_condition<'args>(
    query: &mut QueryBuilder<'args, Postgres>,
    identity_id: Id,
    scope: &'args PackVisibilityScope,
) {
    query.push("(");
    let mut wrote = false;

    match scope.owner {
        Some(OwnerConstraint::SelfOnly) => {
            query.push("installed_by = ");
            query.push_bind(identity_id);
            wrote = true;
        }
        Some(OwnerConstraint::None) => {
            query.push("installed_by IS NULL");
            wrote = true;
        }
        Some(OwnerConstraint::Any) | None => {}
    }
    if let Some(pack_refs) = &scope.pack_refs {
        if wrote {
            query.push(" AND ");
        }
        query.push("ref = ANY(");
        query.push_bind(pack_refs);
        query.push(")");
        wrote = true;
    }
    if let Some(refs) = &scope.refs {
        if wrote {
            query.push(" AND ");
        }
        query.push("ref = ANY(");
        query.push_bind(refs);
        query.push(")");
        wrote = true;
    }
    if let Some(ids) = &scope.ids {
        if wrote {
            query.push(" AND ");
        }
        query.push("id = ANY(");
        query.push_bind(ids);
        query.push(")");
        wrote = true;
    }

    if !wrote {
        query.push("TRUE");
    }
    query.push(")");
}

fn push_pack_scopes_or<'args>(
    query: &mut QueryBuilder<'args, Postgres>,
    identity_id: Id,
    scopes: &'args [PackVisibilityScope],
) {
    if scopes.is_empty() {
        query.push("FALSE");
        return;
    }
    for (index, scope) in scopes.iter().enumerate() {
        if index > 0 {
            query.push(" OR ");
        }
        push_pack_scope_condition(query, identity_id, scope);
    }
}

/// Appends a SQL-side RBAC visibility predicate built from `visibility`.
///
/// `None` applies no restriction.
fn push_pack_visibility_filter<'args>(
    query: &mut QueryBuilder<'args, Postgres>,
    visibility: Option<&'args PackVisibilityFilter>,
) {
    let Some(visibility) = visibility else {
        return;
    };

    query.push(" AND (is_standard OR ((installed_by IS NULL OR installed_by = ");
    query.push_bind(visibility.identity_id);
    query.push(") AND (");
    push_pack_scopes_or(
        query,
        visibility.identity_id,
        &visibility.own_or_ownerless_scopes,
    );
    query.push(")) OR (installed_by IS NOT NULL AND installed_by <> ");
    query.push_bind(visibility.identity_id);
    query.push(" AND (");
    push_pack_scopes_or(
        query,
        visibility.identity_id,
        &visibility.other_owner_scopes,
    );
    query.push(")))");
}

impl PackRepository {
    pub async fn update_worker_placement<'e, E>(
        executor: E,
        id: i64,
        worker_selector: &JsonDict,
        worker_tolerations: &JsonDict,
        worker_affinity: &JsonDict,
    ) -> Result<Pack>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        crate::scheduling::parse_worker_selector(worker_selector)?;
        crate::scheduling::parse_worker_tolerations(worker_tolerations)?;
        crate::scheduling::parse_worker_affinity(worker_affinity)?;
        let query = format!(
            "UPDATE pack SET worker_selector = $2, worker_tolerations = $3, worker_affinity = $4, updated = NOW() WHERE id = $1 RETURNING {}",
            PACK_COLUMNS
        );
        sqlx::query_as::<_, Pack>(&query)
            .bind(id)
            .bind(worker_selector)
            .bind(worker_tolerations)
            .bind(worker_affinity)
            .fetch_one(executor)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => Error::not_found("pack", "id", id.to_string()),
                _ => e.into(),
            })
    }

    /// Serializes filesystem and metadata mutations for one canonical pack ref.
    ///
    /// The lock key is the first 64 bits of SHA-256 over a domain separator and
    /// the validated pack ref. PostgreSQL releases the lock automatically when
    /// `transaction` commits or rolls back. Hash collisions are theoretically
    /// possible, but impractical; a collision would only cause extra serialization.
    pub async fn acquire_mutation_lock(
        transaction: &mut sqlx::Transaction<'_, Postgres>,
        pack_ref: &str,
    ) -> Result<()> {
        RefValidator::validate_pack_ref(pack_ref)?;

        let mut digest = Sha256::new();
        digest.update(b"attune:pack-mutation:v1\0");
        digest.update(pack_ref.as_bytes());
        let key = i64::from_be_bytes(digest.finalize()[..8].try_into().expect("eight-byte slice"));

        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(key)
            .execute(&mut **transaction)
            .await?;
        Ok(())
    }

    /// Lists packs with pagination, optionally restricted by a SQL-side RBAC
    /// visibility filter so totals and pagination stay accurate without
    /// fetching the entire table into memory.
    pub async fn list_search<'e, E>(
        executor: E,
        filters: &PackSearchFilters,
    ) -> Result<PackSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let limit = if filters.limit <= 0 {
            50
        } else {
            filters.limit
        };
        let offset = filters.offset.max(0);

        let mut query = QueryBuilder::new("SELECT ");
        query.push(PACK_COLUMNS);
        query.push(" FROM pack WHERE 1=1");
        push_pack_visibility_filter(&mut query, filters.visibility.as_ref());
        for pattern in text_search_patterns(filters.query.as_deref()) {
            query.push(" AND (LOWER(ref) LIKE ");
            query.push_bind(pattern.clone());
            query.push(" ESCAPE '\\' OR LOWER(label) LIKE ");
            query.push_bind(pattern.clone());
            query.push(" ESCAPE '\\' OR LOWER(COALESCE(description, '')) LIKE ");
            query.push_bind(pattern);
            query.push(" ESCAPE '\\')");
        }
        query.push(" ORDER BY ref ASC LIMIT ");
        query.push_bind(limit);
        query.push(" OFFSET ");
        query.push_bind(offset);

        let rows = query.build_query_as::<Pack>().fetch_all(executor).await?;

        let mut count_query = QueryBuilder::new("SELECT COUNT(*) FROM pack WHERE 1=1");
        push_pack_visibility_filter(&mut count_query, filters.visibility.as_ref());
        for pattern in text_search_patterns(filters.query.as_deref()) {
            count_query.push(" AND (LOWER(ref) LIKE ");
            count_query.push_bind(pattern.clone());
            count_query.push(" ESCAPE '\\' OR LOWER(label) LIKE ");
            count_query.push_bind(pattern.clone());
            count_query.push(" ESCAPE '\\' OR LOWER(COALESCE(description, '')) LIKE ");
            count_query.push_bind(pattern);
            count_query.push(" ESCAPE '\\')");
        }
        let total: i64 = count_query.build_query_scalar().fetch_one(executor).await?;

        Ok(PackSearchResult {
            rows,
            total: total as u64,
        })
    }

    /// List packs with pagination
    pub async fn list_paginated<'e, E>(executor: E, pagination: Pagination) -> Result<Vec<Pack>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM pack ORDER BY ref ASC LIMIT $1 OFFSET $2",
            PACK_COLUMNS
        );
        let packs = sqlx::query_as::<_, Pack>(&query)
            .bind(pagination.limit())
            .bind(pagination.offset())
            .fetch_all(executor)
            .await?;

        Ok(packs)
    }

    /// Count total number of packs
    pub async fn count<'e, E>(executor: E) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pack")
            .fetch_one(executor)
            .await?;

        Ok(count.0)
    }

    /// Stamp the identity that created or installed this pack.
    pub async fn set_installed_by<'e, E>(executor: E, id: i64, installed_by: i64) -> Result<Pack>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "UPDATE pack SET installed_by = $2, updated = NOW() WHERE id = $1 RETURNING {}",
            PACK_COLUMNS
        );
        sqlx::query_as::<_, Pack>(&query)
            .bind(id)
            .bind(installed_by)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    /// Find packs by tag
    pub async fn find_by_tag<'e, E>(executor: E, tag: &str) -> Result<Vec<Pack>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM pack WHERE $1 = ANY(tags) ORDER BY ref ASC",
            PACK_COLUMNS
        );
        let packs = sqlx::query_as::<_, Pack>(&query)
            .bind(tag)
            .fetch_all(executor)
            .await?;

        Ok(packs)
    }

    /// Resolve a list of pack refs to their IDs in a single query.
    /// Returns a map from ref → id; missing refs are simply absent from the map.
    pub async fn find_ids_by_refs<'e, E>(
        executor: E,
        refs: &[&str],
    ) -> Result<std::collections::HashMap<String, i64>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if refs.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let owned: Vec<String> = refs.iter().map(|s| (*s).to_string()).collect();
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT ref, id FROM pack WHERE ref = ANY($1)")
                .bind(&owned)
                .fetch_all(executor)
                .await?;
        Ok(rows.into_iter().collect())
    }

    /// Find standard packs
    pub async fn find_standard<'e, E>(executor: E) -> Result<Vec<Pack>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM pack WHERE is_standard = true ORDER BY ref ASC",
            PACK_COLUMNS
        );
        let packs = sqlx::query_as::<_, Pack>(&query)
            .fetch_all(executor)
            .await?;

        Ok(packs)
    }

    /// Search packs by name/label (case-insensitive)
    pub async fn search<'e, E>(executor: E, query: &str) -> Result<Vec<Pack>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let search_pattern = format!("%{}%", query.to_lowercase());
        let sql = format!(
            "SELECT {} FROM pack WHERE LOWER(ref) LIKE $1 OR LOWER(label) LIKE $1 OR LOWER(description) LIKE $1 ORDER BY ref ASC",
            PACK_COLUMNS
        );
        let packs = sqlx::query_as::<_, Pack>(&sql)
            .bind(&search_pattern)
            .fetch_all(executor)
            .await?;

        Ok(packs)
    }

    /// Check if a pack with the given ref exists
    pub async fn exists_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let exists: (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pack WHERE ref = $1)")
            .bind(ref_str)
            .fetch_one(executor)
            .await?;

        Ok(exists.0)
    }

    /// Update installation metadata for a pack
    #[allow(clippy::too_many_arguments)]
    pub async fn update_installation_metadata<'e, E>(
        executor: E,
        id: i64,
        source_type: String,
        source_url: Option<String>,
        source_ref: Option<String>,
        checksum: Option<String>,
        checksum_verified: bool,
        installed_by: Option<i64>,
        installation_method: String,
        storage_path: String,
    ) -> Result<Pack>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            r#"
            UPDATE pack
            SET source_type = $2,
                source_url = $3,
                source_ref = $4,
                checksum = $5,
                checksum_verified = $6,
                installed_at = NOW(),
                installed_by = $7,
                installation_method = $8,
                storage_path = $9,
                updated = NOW()
            WHERE id = $1
            RETURNING {}
            "#,
            PACK_COLUMNS
        );
        let pack = sqlx::query_as::<_, Pack>(&query)
            .bind(id)
            .bind(source_type)
            .bind(source_url)
            .bind(source_ref)
            .bind(checksum)
            .bind(checksum_verified)
            .bind(installed_by)
            .bind(installation_method)
            .bind(storage_path)
            .fetch_one(executor)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => Error::not_found("pack", "id", id.to_string()),
                _ => e.into(),
            })?;

        Ok(pack)
    }

    /// Check if a pack has installation metadata
    pub async fn is_installed<'e, E>(executor: E, pack_id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let exists: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM pack WHERE id = $1 AND installed_at IS NOT NULL)",
        )
        .bind(pack_id)
        .fetch_one(executor)
        .await?;

        Ok(exists.0)
    }

    /// List all installed packs
    pub async fn list_installed<'e, E>(executor: E) -> Result<Vec<Pack>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM pack WHERE installed_at IS NOT NULL ORDER BY installed_at DESC",
            PACK_COLUMNS
        );
        let packs = sqlx::query_as::<_, Pack>(&query)
            .fetch_all(executor)
            .await?;

        Ok(packs)
    }

    /// List packs by source type
    pub async fn list_by_source_type<'e, E>(executor: E, source_type: &str) -> Result<Vec<Pack>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM pack WHERE source_type = $1 ORDER BY installed_at DESC",
            PACK_COLUMNS
        );
        let packs = sqlx::query_as::<_, Pack>(&query)
            .bind(source_type)
            .fetch_all(executor)
            .await?;

        Ok(packs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_pack_input() {
        let input = CreatePackInput {
            r#ref: "test_pack".to_string(),
            label: "Test Pack".to_string(),
            description: Some("A test pack".to_string()),
            version: "1.0.0".to_string(),
            conf_schema: serde_json::json!({}),
            config: serde_json::json!({}),
            meta: serde_json::json!({}),
            tags: vec!["test".to_string()],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: serde_json::json!({}),
        };

        assert_eq!(input.r#ref, "test_pack");
        assert_eq!(input.label, "Test Pack");
    }

    #[test]
    fn test_update_pack_input_default() {
        let input = UpdatePackInput::default();
        assert!(input.label.is_none());
        assert!(input.description.is_none());
        assert!(input.version.is_none());
        assert!(input.dependencies.is_none());
    }
}
