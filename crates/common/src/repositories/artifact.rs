//! Artifact and ArtifactVersion repositories for database operations

use crate::artifact_transport::ValidatedRelativePath;
use crate::models::{
    artifact::*,
    artifact_version::ArtifactVersion,
    enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
};
use crate::rbac::{Action, ExecutionScopeConstraint, Grant, OwnerConstraint, Resource};
use crate::Result;
use sqlx::{Executor, Postgres, QueryBuilder};
use std::collections::HashMap;

use super::{Create, Delete, FindById, FindByRef, List, Patch, Repository, Update};

// ============================================================================
// ArtifactRepository
// ============================================================================

pub struct ArtifactRepository;

impl Repository for ArtifactRepository {
    type Entity = Artifact;
    fn table_name() -> &'static str {
        "artifact"
    }
}

#[derive(Debug, Clone)]
pub struct CreateArtifactInput {
    pub r#ref: String,
    pub scope: OwnerType,
    pub owner: String,
    pub r#type: ArtifactType,
    pub visibility: ArtifactVisibility,
    pub classification: ArtifactClassification,
    pub retention_policy: RetentionPolicyType,
    pub retention_limit: i32,
    pub name: Option<String>,
    pub description: Option<String>,
    pub content_type: Option<String>,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateArtifactInput {
    pub r#ref: Option<String>,
    pub scope: Option<OwnerType>,
    pub owner: Option<String>,
    pub r#type: Option<ArtifactType>,
    pub visibility: Option<ArtifactVisibility>,
    pub classification: Option<ArtifactClassification>,
    pub retention_policy: Option<RetentionPolicyType>,
    pub retention_limit: Option<i32>,
    pub name: Option<Patch<String>>,
    pub description: Option<Patch<String>>,
    pub content_type: Option<Patch<String>>,
    pub size_bytes: Option<i64>,
    pub data: Option<Patch<serde_json::Value>>,
}

/// Filters for searching artifacts
#[derive(Debug, Clone, Default)]
pub struct ArtifactSearchFilters {
    pub scope: Option<OwnerType>,
    pub owner: Option<String>,
    pub r#type: Option<ArtifactType>,
    pub visibility: Option<ArtifactVisibility>,
    pub classification: Option<ArtifactClassification>,
    /// Filter to artifacts that have at least one version produced by this
    /// execution. Implemented by joining through `artifact_version`.
    pub execution: Option<i64>,
    pub name_contains: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

/// Search result with total count
pub struct ArtifactSearchResult {
    pub rows: Vec<Artifact>,
    pub total: i64,
}

#[derive(Debug, Clone)]
pub struct ArtifactReadContext {
    pub identity_id: i64,
    pub identity_attributes: HashMap<String, serde_json::Value>,
    pub grants: Vec<Grant>,
}

#[async_trait::async_trait]
impl FindById for ArtifactRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM artifact WHERE id = $1", SELECT_COLUMNS);
        sqlx::query_as::<_, Artifact>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl FindByRef for ArtifactRepository {
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!("SELECT {} FROM artifact WHERE ref = $1", SELECT_COLUMNS);
        sqlx::query_as::<_, Artifact>(&query)
            .bind(ref_str)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for ArtifactRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact ORDER BY created DESC, id DESC LIMIT 1000",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for ArtifactRepository {
    type CreateInput = CreateArtifactInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        validate_artifact_ref(&input.r#ref)?;
        let query = format!(
            "INSERT INTO artifact (ref, scope, owner, type, visibility, classification, retention_policy, retention_limit, \
             name, description, content_type, data) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             RETURNING {}",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(&input.r#ref)
            .bind(input.scope)
            .bind(&input.owner)
            .bind(input.r#type)
            .bind(input.visibility)
            .bind(input.classification)
            .bind(input.retention_policy)
            .bind(input.retention_limit)
            .bind(&input.name)
            .bind(&input.description)
            .bind(&input.content_type)
            .bind(&input.data)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for ArtifactRepository {
    type UpdateInput = UpdateArtifactInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if let Some(artifact_ref) = input.r#ref.as_deref() {
            validate_artifact_ref(artifact_ref)?;
        }
        let mut query = QueryBuilder::new("UPDATE artifact SET ");
        let mut has_updates = false;

        macro_rules! push_field {
            ($field:expr, $col:expr) => {
                if let Some(val) = $field {
                    if has_updates {
                        query.push(", ");
                    }
                    query.push(concat!($col, " = ")).push_bind(val);
                    has_updates = true;
                }
            };
        }

        push_field!(&input.r#ref, "ref");
        push_field!(input.scope, "scope");
        push_field!(&input.owner, "owner");
        push_field!(input.r#type, "type");
        push_field!(input.visibility, "visibility");
        push_field!(input.classification, "classification");
        push_field!(input.retention_policy, "retention_policy");
        push_field!(input.retention_limit, "retention_limit");
        if let Some(name) = &input.name {
            if has_updates {
                query.push(", ");
            }
            query.push("name = ");
            match name {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
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
        if let Some(content_type) = &input.content_type {
            if has_updates {
                query.push(", ");
            }
            query.push("content_type = ");
            match content_type {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<String>::None),
            };
            has_updates = true;
        }
        push_field!(input.size_bytes, "size_bytes");
        if let Some(data) = &input.data {
            if has_updates {
                query.push(", ");
            }
            query.push("data = ");
            match data {
                Patch::Set(value) => query.push_bind(value),
                Patch::Clear => query.push_bind(Option::<serde_json::Value>::None),
            };
            has_updates = true;
        }

        if !has_updates {
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(" RETURNING ");
        query.push(SELECT_COLUMNS);

        query
            .build_query_as::<Artifact>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for ArtifactRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM artifact WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl ArtifactRepository {
    /// Search artifacts with filters and pagination
    pub async fn search<'e, E>(
        executor: E,
        filters: &ArtifactSearchFilters,
    ) -> Result<ArtifactSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        Self::search_internal(executor, filters, None).await
    }

    /// Search artifacts with read visibility applied in-database.
    pub async fn search_readable<'e, E>(
        executor: E,
        filters: &ArtifactSearchFilters,
        read_ctx: &ArtifactReadContext,
    ) -> Result<ArtifactSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        Self::search_internal(executor, filters, Some(read_ctx)).await
    }

    pub async fn is_readable<'e, E>(
        executor: E,
        artifact_id: i64,
        read_ctx: &ArtifactReadContext,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query =
            QueryBuilder::<Postgres>::new("SELECT EXISTS (SELECT 1 FROM artifact a WHERE a.id = ");
        query.push_bind(artifact_id);
        query.push(" AND ");
        push_artifact_read_predicate(&mut query, read_ctx, "a");
        query.push(")");

        query
            .build_query_scalar::<bool>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    async fn search_internal<'e, E>(
        executor: E,
        filters: &ArtifactSearchFilters,
        read_ctx: Option<&ArtifactReadContext>,
    ) -> Result<ArtifactSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let mut count_query = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM artifact a");
        let mut has_where = false;
        push_artifact_filters(&mut count_query, filters, "a", &mut has_where);
        if let Some(ctx) = read_ctx {
            push_where_prefix(&mut count_query, &mut has_where);
            push_artifact_read_predicate(&mut count_query, ctx, "a");
        }
        let total = count_query
            .build_query_scalar::<i64>()
            .fetch_one(executor)
            .await?;

        let mut data_query =
            QueryBuilder::<Postgres>::new(format!("SELECT {} FROM artifact a", SELECT_COLUMNS));
        let mut has_where = false;
        push_artifact_filters(&mut data_query, filters, "a", &mut has_where);
        if let Some(ctx) = read_ctx {
            push_where_prefix(&mut data_query, &mut has_where);
            push_artifact_read_predicate(&mut data_query, ctx, "a");
        }

        let limit = filters.limit.min(1000);
        data_query.push(" ORDER BY a.created DESC, a.id DESC LIMIT ");
        data_query.push_bind(limit as i64);
        data_query.push(" OFFSET ");
        data_query.push_bind(filters.offset as i64);

        let rows = data_query
            .build_query_as::<Artifact>()
            .fetch_all(executor)
            .await?;

        Ok(ArtifactSearchResult { rows, total })
    }

    /// Find artifacts by scope
    pub async fn find_by_scope<'e, E>(executor: E, scope: OwnerType) -> Result<Vec<Artifact>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact WHERE scope = $1 ORDER BY created DESC, id DESC",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(scope)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Find artifacts by owner
    pub async fn find_by_owner<'e, E>(executor: E, owner: &str) -> Result<Vec<Artifact>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact WHERE owner = $1 ORDER BY created DESC, id DESC",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(owner)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Find artifacts by type
    pub async fn find_by_type<'e, E>(
        executor: E,
        artifact_type: ArtifactType,
    ) -> Result<Vec<Artifact>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact WHERE type = $1 ORDER BY created DESC, id DESC",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(artifact_type)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Find artifacts by scope and owner
    pub async fn find_by_scope_and_owner<'e, E>(
        executor: E,
        scope: OwnerType,
        owner: &str,
    ) -> Result<Vec<Artifact>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact WHERE scope = $1 AND owner = $2 ORDER BY created DESC, id DESC",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(scope)
            .bind(owner)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Find artifacts that have at least one version produced by the given execution.
    /// Uses a JOIN through `artifact_version` (per-version `execution` column).
    pub async fn find_by_execution<'e, E>(executor: E, execution_id: i64) -> Result<Vec<Artifact>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let select_with_alias = SELECT_COLUMNS
            .split(',')
            .map(|c| format!("a.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT DISTINCT {} FROM artifact a \
             JOIN artifact_version av ON av.artifact = a.id \
             WHERE av.execution = $1 \
             ORDER BY a.created DESC, a.id DESC",
            select_with_alias
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(execution_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Find artifacts by retention policy
    pub async fn find_by_retention_policy<'e, E>(
        executor: E,
        retention_policy: RetentionPolicyType,
    ) -> Result<Vec<Artifact>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact WHERE retention_policy = $1 ORDER BY created DESC, id DESC",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(retention_policy)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Append data to a progress-type artifact.
    ///
    /// If `artifact.data` is currently NULL, it is initialized as a JSON array
    /// containing the new entry. Otherwise the entry is appended to the existing
    /// array. This is done atomically in a single SQL statement.
    pub async fn append_progress<'e, E>(
        executor: E,
        id: i64,
        entry: &serde_json::Value,
    ) -> Result<Artifact>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "UPDATE artifact \
             SET data = CASE \
                 WHEN data IS NULL THEN jsonb_build_array($2::jsonb) \
                 ELSE data || jsonb_build_array($2::jsonb) \
             END, \
             updated = NOW() \
             WHERE id = $1 AND type = 'progress' \
             RETURNING {}",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(id)
            .bind(entry)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    /// Replace the full data payload on a progress-type artifact (for "set" semantics).
    pub async fn set_data<'e, E>(executor: E, id: i64, data: &serde_json::Value) -> Result<Artifact>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "UPDATE artifact SET data = $2, updated = NOW() \
             WHERE id = $1 RETURNING {}",
            SELECT_COLUMNS
        );
        sqlx::query_as::<_, Artifact>(&query)
            .bind(id)
            .bind(data)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    /// Update the size_bytes of an artifact (used by worker finalization to sync
    /// the parent artifact's size with the latest file-based version).
    pub async fn update_size_bytes<'e, E>(executor: E, id: i64, size_bytes: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result =
            sqlx::query("UPDATE artifact SET size_bytes = $1, updated = NOW() WHERE id = $2")
                .bind(size_bytes)
                .bind(id)
                .execute(executor)
                .await?;
        Ok(result.rows_affected() > 0)
    }
}

fn grant_attributes_match(
    grant: &Grant,
    identity_attributes: &HashMap<String, serde_json::Value>,
) -> bool {
    let Some(constraints) = &grant.constraints else {
        return true;
    };
    let Some(attributes) = &constraints.attributes else {
        return true;
    };
    attributes
        .iter()
        .all(|(key, expected)| identity_attributes.get(key) == Some(expected))
}

fn push_where_prefix<'a>(query: &mut QueryBuilder<'a, Postgres>, has_where: &mut bool) {
    if *has_where {
        query.push(" AND ");
    } else {
        query.push(" WHERE ");
        *has_where = true;
    }
}

fn push_artifact_filters<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    filters: &ArtifactSearchFilters,
    alias: &str,
    has_where: &mut bool,
) {
    if let Some(scope) = filters.scope {
        push_where_prefix(query, has_where);
        query.push(alias).push(".scope = ").push_bind(scope);
    }
    if let Some(owner) = &filters.owner {
        push_where_prefix(query, has_where);
        query.push(alias).push(".owner = ").push_bind(owner.clone());
    }
    if let Some(artifact_type) = filters.r#type {
        push_where_prefix(query, has_where);
        query.push(alias).push(".type = ").push_bind(artifact_type);
    }
    if let Some(visibility) = filters.visibility {
        push_where_prefix(query, has_where);
        query
            .push(alias)
            .push(".visibility = ")
            .push_bind(visibility);
    }
    if let Some(classification) = filters.classification {
        push_where_prefix(query, has_where);
        query
            .push(alias)
            .push(".classification = ")
            .push_bind(classification);
    }
    if let Some(execution) = filters.execution {
        push_where_prefix(query, has_where);
        query.push("EXISTS (SELECT 1 FROM artifact_version av WHERE av.artifact = ");
        query.push(alias).push(".id AND av.execution = ");
        query.push_bind(execution);
        query.push(")");
    }
    if let Some(name_contains) = &filters.name_contains {
        push_where_prefix(query, has_where);
        query.push(alias).push(".name ILIKE '%' || ");
        query.push_bind(name_contains.clone());
        query.push(" || '%'");
    }
}

fn artifact_pack_ref_expr(alias: &str) -> String {
    format!(
        "CASE \
            WHEN {alias}.scope = 'pack' THEN {alias}.owner \
            WHEN {alias}.scope IN ('action', 'sensor') THEN split_part({alias}.owner, '.', 1) \
            ELSE NULL \
        END"
    )
}

fn artifact_owner_identity_expr(alias: &str) -> String {
    format!(
        "CASE \
            WHEN {alias}.scope = 'identity' AND {alias}.owner ~ '^[0-9]+$' THEN {alias}.owner::bigint \
            ELSE NULL \
        END"
    )
}

fn execution_owner_identity_expr(alias: &str) -> String {
    format!("{alias}.executor")
}

fn execution_pack_ref_expr(alias: &str) -> String {
    format!("split_part({alias}.action_ref, '.', 1)")
}

fn push_artifact_read_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    read_ctx: &ArtifactReadContext,
    artifact_alias: &str,
) {
    query.push("(");
    push_artifact_grants_predicate(query, read_ctx, artifact_alias, true);
    query.push(" OR ");
    query.push("(");
    query.push("EXISTS (SELECT 1 FROM artifact_version av_link WHERE av_link.artifact = ");
    query.push(artifact_alias);
    query.push(".id AND av_link.execution IS NOT NULL)");
    query.push(" AND EXISTS (SELECT 1 FROM artifact_version av_link JOIN execution e ON e.id = av_link.execution WHERE av_link.artifact = ");
    query.push(artifact_alias);
    query.push(".id AND ");
    push_execution_readable_predicate(query, read_ctx, "e");
    query.push(")");
    query.push(")");
    query.push(" OR ");
    query.push("(");
    query.push("NOT EXISTS (SELECT 1 FROM artifact_version av_link WHERE av_link.artifact = ");
    query.push(artifact_alias);
    query.push(".id AND av_link.execution IS NOT NULL)");
    query.push(" AND ");
    push_owner_path_read_predicate(query, read_ctx, artifact_alias);
    query.push(")");
    query.push(")");
}

fn push_owner_path_read_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    read_ctx: &ArtifactReadContext,
    artifact_alias: &str,
) {
    query.push("(");
    query.push("(");
    query.push(artifact_alias).push(".visibility = 'public'");
    query.push(")");
    query.push(" OR ");
    query.push("(");
    query
        .push(artifact_alias)
        .push(".visibility = 'private' AND (");
    push_artifact_grants_predicate(query, read_ctx, artifact_alias, true);
    query.push(" OR (");
    query.push(artifact_alias).push(".scope = 'identity' AND ");
    query.push(artifact_alias).push(".owner = ");
    query.push_bind(read_ctx.identity_id.to_string());
    query.push(")");
    query.push(" OR (");
    query
        .push(artifact_alias)
        .push(".scope IN ('pack', 'action', 'sensor') AND ");
    let pack_expr = artifact_pack_ref_expr(artifact_alias);
    push_pack_grants_predicate(query, read_ctx, &pack_expr);
    query.push(")");
    query.push(")");
    query.push(")");
    query.push(")");
}

fn can_apply_artifact_grant(grant: &Grant) -> bool {
    let Some(constraints) = &grant.constraints else {
        return true;
    };
    constraints.execution_scope.is_none() && constraints.encrypted.is_none()
}

fn push_artifact_grants_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    read_ctx: &ArtifactReadContext,
    artifact_alias: &str,
    require_constraints: bool,
) {
    let grants: Vec<&Grant> = read_ctx
        .grants
        .iter()
        .filter(|grant| {
            grant.resource == Resource::Artifacts
                && grant.actions.contains(&Action::Read)
                && grant_attributes_match(grant, &read_ctx.identity_attributes)
                && (!require_constraints || grant.constraints.is_some())
        })
        .collect();

    let mut wrote_any = false;
    query.push("(");
    for grant in grants {
        if !can_apply_artifact_grant(grant) {
            continue;
        }
        if wrote_any {
            query.push(" OR ");
        }
        push_single_artifact_grant_predicate(query, grant, read_ctx, artifact_alias);
        wrote_any = true;
    }
    if !wrote_any {
        query.push("FALSE");
    }
    query.push(")");
}

fn push_single_artifact_grant_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    grant: &Grant,
    read_ctx: &ArtifactReadContext,
    artifact_alias: &str,
) {
    let Some(constraints) = &grant.constraints else {
        query.push("TRUE");
        return;
    };

    query.push("(");

    let mut has_term = false;
    let push_and = |query: &mut QueryBuilder<'a, Postgres>, has_term: &mut bool| {
        if *has_term {
            query.push(" AND ");
        } else {
            *has_term = true;
        }
    };

    if let Some(pack_refs) = &constraints.pack_refs {
        if pack_refs.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        let pack_expr = artifact_pack_ref_expr(artifact_alias);
        query.push("(").push(&pack_expr).push(" = ANY(");
        query.push_bind(pack_refs.clone());
        query.push("))");
    }

    if let Some(owner) = constraints.owner {
        push_and(query, &mut has_term);
        let owner_expr = artifact_owner_identity_expr(artifact_alias);
        match owner {
            OwnerConstraint::SelfOnly => {
                query.push("(").push(&owner_expr).push(" = ");
                query.push_bind(read_ctx.identity_id);
                query.push(")");
            }
            OwnerConstraint::Any => {
                query.push("TRUE");
            }
            OwnerConstraint::None => {
                query.push("(").push(&owner_expr).push(" IS NULL)");
            }
        }
    }

    if let Some(owner_types) = &constraints.owner_types {
        if owner_types.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(").push(artifact_alias).push(".scope IN (");
        {
            let mut separated = query.separated(", ");
            for owner_type in owner_types {
                separated.push_bind(*owner_type);
            }
        }
        query.push("))");
    }

    if let Some(owner_refs) = &constraints.owner_refs {
        if owner_refs.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(").push(artifact_alias).push(".owner = ANY(");
        query.push_bind(owner_refs.clone());
        query.push("))");
    }

    if let Some(visibility) = &constraints.visibility {
        if visibility.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query
            .push("(")
            .push(artifact_alias)
            .push(".visibility IN (");
        {
            let mut separated = query.separated(", ");
            for item in visibility {
                separated.push_bind(*item);
            }
        }
        query.push("))");
    }

    if let Some(refs) = &constraints.refs {
        if refs.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(").push(artifact_alias).push(".ref = ANY(");
        query.push_bind(refs.clone());
        query.push("))");
    }

    if let Some(ids) = &constraints.ids {
        if ids.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(").push(artifact_alias).push(".id = ANY(");
        query.push_bind(ids.clone());
        query.push("))");
    }

    if !has_term {
        query.push("TRUE");
    }
    query.push(")");
}

fn can_apply_pack_grant(grant: &Grant) -> bool {
    let Some(constraints) = &grant.constraints else {
        return true;
    };

    constraints.owner_types.is_none()
        && constraints.owner_refs.is_none()
        && constraints.visibility.is_none()
        && constraints.execution_scope.is_none()
        && constraints.ids.is_none()
        && constraints.encrypted.is_none()
        && !matches!(constraints.owner, Some(OwnerConstraint::SelfOnly))
}

fn push_pack_grants_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    read_ctx: &ArtifactReadContext,
    pack_expr: &str,
) {
    let grants: Vec<&Grant> = read_ctx
        .grants
        .iter()
        .filter(|grant| {
            grant.resource == Resource::Packs
                && grant.actions.contains(&Action::Read)
                && grant_attributes_match(grant, &read_ctx.identity_attributes)
        })
        .collect();

    let mut wrote_any = false;
    query.push("(");
    for grant in grants {
        if !can_apply_pack_grant(grant) {
            continue;
        }
        if wrote_any {
            query.push(" OR ");
        }
        push_single_pack_grant_predicate(query, grant, pack_expr);
        wrote_any = true;
    }
    if !wrote_any {
        query.push("FALSE");
    }
    query.push(")");
}

fn push_single_pack_grant_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    grant: &Grant,
    pack_expr: &str,
) {
    let Some(constraints) = &grant.constraints else {
        query.push("TRUE");
        return;
    };

    query.push("(");
    let mut has_term = false;
    let push_and = |query: &mut QueryBuilder<'a, Postgres>, has_term: &mut bool| {
        if *has_term {
            query.push(" AND ");
        } else {
            *has_term = true;
        }
    };

    if let Some(pack_refs) = &constraints.pack_refs {
        if pack_refs.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(").push(pack_expr).push(" = ANY(");
        query.push_bind(pack_refs.clone());
        query.push("))");
    }

    if let Some(refs) = &constraints.refs {
        if refs.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(").push(pack_expr).push(" = ANY(");
        query.push_bind(refs.clone());
        query.push("))");
    }

    if !has_term {
        query.push("TRUE");
    }
    query.push(")");
}

fn can_apply_execution_grant(grant: &Grant) -> bool {
    let Some(constraints) = &grant.constraints else {
        return true;
    };
    constraints.owner_types.is_none()
        && constraints.owner_refs.is_none()
        && constraints.visibility.is_none()
        && constraints.encrypted.is_none()
}

fn push_execution_readable_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    read_ctx: &ArtifactReadContext,
    execution_alias: &str,
) {
    let grants: Vec<&Grant> = read_ctx
        .grants
        .iter()
        .filter(|grant| {
            grant.resource == Resource::Executions
                && grant.actions.contains(&Action::Read)
                && grant_attributes_match(grant, &read_ctx.identity_attributes)
        })
        .collect();

    let mut wrote_any = false;
    query.push("(");
    for grant in grants {
        if !can_apply_execution_grant(grant) {
            continue;
        }
        if wrote_any {
            query.push(" OR ");
        }
        push_single_execution_grant_predicate(query, grant, read_ctx, execution_alias);
        wrote_any = true;
    }
    if !wrote_any {
        query.push("FALSE");
    }
    query.push(")");
}

fn push_single_execution_grant_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    grant: &Grant,
    read_ctx: &ArtifactReadContext,
    execution_alias: &str,
) {
    let Some(constraints) = &grant.constraints else {
        query.push("TRUE");
        return;
    };

    query.push("(");
    let mut has_term = false;
    let push_and = |query: &mut QueryBuilder<'a, Postgres>, has_term: &mut bool| {
        if *has_term {
            query.push(" AND ");
        } else {
            *has_term = true;
        }
    };

    if let Some(pack_refs) = &constraints.pack_refs {
        if pack_refs.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        let pack_expr = execution_pack_ref_expr(execution_alias);
        query.push("(").push(&pack_expr).push(" = ANY(");
        query.push_bind(pack_refs.clone());
        query.push("))");
    }

    if let Some(owner) = constraints.owner {
        push_and(query, &mut has_term);
        let owner_expr = execution_owner_identity_expr(execution_alias);
        match owner {
            OwnerConstraint::SelfOnly => {
                query.push("(").push(&owner_expr).push(" = ");
                query.push_bind(read_ctx.identity_id);
                query.push(")");
            }
            OwnerConstraint::Any => {
                query.push("TRUE");
            }
            OwnerConstraint::None => {
                query.push("(").push(&owner_expr).push(" IS NULL)");
            }
        }
    }

    if let Some(refs) = &constraints.refs {
        if refs.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query
            .push("(")
            .push(execution_alias)
            .push(".action_ref = ANY(");
        query.push_bind(refs.clone());
        query.push("))");
    }

    if let Some(ids) = &constraints.ids {
        if ids.is_empty() {
            query.push("FALSE");
            query.push(")");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(").push(execution_alias).push(".id = ANY(");
        query.push_bind(ids.clone());
        query.push("))");
    }

    if let Some(scope) = constraints.execution_scope {
        push_and(query, &mut has_term);
        match scope {
            ExecutionScopeConstraint::SelfOnly => {
                query.push("(").push(execution_alias).push(".executor = ");
                query.push_bind(read_ctx.identity_id);
                query.push(")");
            }
            ExecutionScopeConstraint::Descendants => {
                query.push("(").push(execution_alias).push(".executor = ");
                query.push_bind(read_ctx.identity_id);
                query.push(" OR ");
                push_execution_descendants_predicate(query, execution_alias, read_ctx.identity_id);
                query.push(")");
            }
            ExecutionScopeConstraint::Any => {
                query.push("TRUE");
            }
        }
    }

    if !has_term {
        query.push("TRUE");
    }
    query.push(")");
}

fn push_execution_descendants_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    execution_alias: &str,
    identity_id: i64,
) {
    query.push("EXISTS (WITH RECURSIVE execution_ancestors AS (");
    query.push("SELECT p.id, p.parent, p.executor, 1 AS depth FROM execution p WHERE p.id = ");
    query.push(execution_alias).push(".parent");
    query.push(" UNION ALL ");
    query.push("SELECT p2.id, p2.parent, p2.executor, ea.depth + 1 ");
    query.push("FROM execution p2 JOIN execution_ancestors ea ON ea.parent = p2.id ");
    query.push("WHERE ea.parent IS NOT NULL AND ea.depth < 64");
    query.push(") SELECT 1 FROM execution_ancestors WHERE executor = ");
    query.push_bind(identity_id);
    query.push(" LIMIT 1)");
}

fn push_artifact_version_read_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    read_ctx: &ArtifactReadContext,
    artifact_alias: &str,
    version_alias: &str,
) {
    query.push("(");
    push_artifact_grants_predicate(query, read_ctx, artifact_alias, true);
    query.push(" OR ");
    query.push("(");
    query.push("EXISTS (SELECT 1 FROM artifact_version av_link WHERE av_link.artifact = ");
    query.push(artifact_alias);
    query.push(".id AND av_link.execution IS NOT NULL)");
    query.push(" AND ");
    query.push(version_alias).push(".execution IS NOT NULL");
    query.push(" AND EXISTS (SELECT 1 FROM execution e WHERE e.id = ");
    query.push(version_alias).push(".execution AND ");
    push_execution_readable_predicate(query, read_ctx, "e");
    query.push(")");
    query.push(")");
    query.push(" OR ");
    query.push("(");
    query.push("NOT EXISTS (SELECT 1 FROM artifact_version av_link WHERE av_link.artifact = ");
    query.push(artifact_alias);
    query.push(".id AND av_link.execution IS NOT NULL)");
    query.push(" AND ");
    push_owner_path_read_predicate(query, read_ctx, artifact_alias);
    query.push(")");
    query.push(")");
}

// ============================================================================
// ArtifactVersionRepository
// ============================================================================

use crate::models::artifact_version;

pub struct ArtifactVersionRepository;

impl Repository for ArtifactVersionRepository {
    type Entity = ArtifactVersion;
    fn table_name() -> &'static str {
        "artifact_version"
    }
}

#[derive(Debug, Clone)]
pub struct CreateArtifactVersionInput {
    pub artifact: i64,
    /// Optional execution that produced this version. Used for per-version
    /// linkage to the originating execution (e.g., per-execution log versions).
    pub execution: Option<i64>,
    pub content_type: Option<String>,
    pub content: Option<Vec<u8>>,
    pub content_json: Option<serde_json::Value>,
    pub file_path: Option<String>,
    pub meta: Option<serde_json::Value>,
    pub created_by: Option<String>,
}

/// Returns true for artifact types that should use file-backed storage on disk.
pub fn is_file_backed_type(artifact_type: ArtifactType) -> bool {
    matches!(
        artifact_type,
        ArtifactType::FileBinary
            | ArtifactType::FileText
            | ArtifactType::FileDataTable
            | ArtifactType::FileImage
    )
}

/// Validate an artifact ref before it can be persisted or converted into a path.
pub fn validate_artifact_ref(artifact_ref: &str) -> Result<()> {
    if artifact_ref.trim() != artifact_ref || artifact_ref.is_empty() {
        return Err(crate::error::Error::Validation(
            "Artifact ref must be non-empty and have no surrounding whitespace".to_string(),
        ));
    }
    if artifact_ref.contains('/') {
        return Err(crate::error::Error::Validation(
            "Artifact ref must use dots, not path separators".to_string(),
        ));
    }
    let derived = artifact_ref.replace('.', "/");
    ValidatedRelativePath::new(&derived)?;
    Ok(())
}

/// Convert a validated artifact ref to a directory path by replacing dots with path separators.
/// e.g., "mypack.build_log" -> "mypack/build_log"
pub fn ref_to_dir_path(artifact_ref: &str) -> Result<String> {
    validate_artifact_ref(artifact_ref)?;
    Ok(artifact_ref.replace('.', "/"))
}

/// Compute the relative file path for a file-backed artifact version.
///
/// Pattern: `{ref_slug}/v{version}.{ext}`
/// e.g., `mypack/build_log/v1.txt`
pub fn compute_file_path(artifact_ref: &str, version: i32, content_type: &str) -> Result<String> {
    let ref_path = ref_to_dir_path(artifact_ref)?;
    let ext = extension_from_content_type(content_type);
    let file_path = format!("{}/v{}.{}", ref_path, version, ext);
    ValidatedRelativePath::new(&file_path)?;
    Ok(file_path)
}

/// Return a sensible default content type for a given artifact type.
pub fn default_content_type_for_artifact(artifact_type: ArtifactType) -> String {
    match artifact_type {
        ArtifactType::FileText => "text/plain".to_string(),
        ArtifactType::FileDataTable => "text/csv".to_string(),
        ArtifactType::FileImage => "image/png".to_string(),
        ArtifactType::FileBinary => "application/octet-stream".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

pub fn is_runtime_log_artifact_ref(artifact_ref: &str) -> bool {
    artifact_ref.ends_with(".stdout.log")
        || artifact_ref.ends_with(".stderr.log")
        || (artifact_ref.starts_with("sensor.")
            && (artifact_ref.ends_with(".stdout") || artifact_ref.ends_with(".stderr")))
}

pub fn classify_artifact(
    artifact_ref: &str,
    artifact_type: ArtifactType,
) -> ArtifactClassification {
    if artifact_type == ArtifactType::FileText && is_runtime_log_artifact_ref(artifact_ref) {
        ArtifactClassification::RuntimeLog
    } else {
        ArtifactClassification::General
    }
}

fn extension_from_content_type(ct: &str) -> &str {
    match ct.split(';').next().unwrap_or("").trim() {
        "text/plain" => "txt",
        "text/csv" => "csv",
        "application/json" => "json",
        "application/yaml" | "text/yaml" => "yaml",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        _ => "bin",
    }
}

impl ArtifactVersionRepository {
    fn select_columns_with_alias(alias: &str) -> String {
        format!(
            "{alias}.id, {alias}.artifact, {alias}.version, {alias}.execution, \
             {alias}.content_type, {alias}.size_bytes, NULL::bytea AS content, \
             {alias}.content_json, {alias}.file_path, {alias}.meta, \
             {alias}.created_by, {alias}.created"
        )
    }

    /// Find a version by ID (without binary content for performance)
    pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version WHERE id = $1",
            artifact_version::SELECT_COLUMNS
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Find a version by ID including binary content
    pub async fn find_by_id_with_content<'e, E>(
        executor: E,
        id: i64,
    ) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version WHERE id = $1",
            artifact_version::SELECT_COLUMNS_WITH_CONTENT
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// List all versions for an artifact (without binary content), newest first
    pub async fn list_by_artifact<'e, E>(
        executor: E,
        artifact_id: i64,
    ) -> Result<Vec<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version WHERE artifact = $1 ORDER BY version DESC",
            artifact_version::SELECT_COLUMNS
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(artifact_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// List readable versions for an artifact (without binary content), newest first.
    pub async fn list_readable_by_artifact<'e, E>(
        executor: E,
        artifact_id: i64,
        read_ctx: &ArtifactReadContext,
    ) -> Result<Vec<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT {} FROM artifact_version av JOIN artifact a ON a.id = av.artifact WHERE av.artifact = ",
            Self::select_columns_with_alias("av")
        ));
        query.push_bind(artifact_id);
        query.push(" AND ");
        push_artifact_version_read_predicate(&mut query, read_ctx, "a", "av");
        query.push(" ORDER BY av.version DESC");

        query
            .build_query_as::<ArtifactVersion>()
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Get the latest version for an artifact (without binary content)
    pub async fn find_latest<'e, E>(
        executor: E,
        artifact_id: i64,
    ) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version WHERE artifact = $1 ORDER BY version DESC LIMIT 1",
            artifact_version::SELECT_COLUMNS
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(artifact_id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Get the latest readable version for an artifact (without binary content).
    pub async fn find_latest_readable<'e, E>(
        executor: E,
        artifact_id: i64,
        read_ctx: &ArtifactReadContext,
    ) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT {} FROM artifact_version av JOIN artifact a ON a.id = av.artifact WHERE av.artifact = ",
            Self::select_columns_with_alias("av")
        ));
        query.push_bind(artifact_id);
        query.push(" AND ");
        push_artifact_version_read_predicate(&mut query, read_ctx, "a", "av");
        query.push(" ORDER BY av.version DESC LIMIT 1");

        query
            .build_query_as::<ArtifactVersion>()
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Get the latest version for an artifact (with binary content)
    pub async fn find_latest_with_content<'e, E>(
        executor: E,
        artifact_id: i64,
    ) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version WHERE artifact = $1 ORDER BY version DESC LIMIT 1",
            artifact_version::SELECT_COLUMNS_WITH_CONTENT
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(artifact_id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Get a specific version by artifact and version number (without binary content)
    pub async fn find_by_version<'e, E>(
        executor: E,
        artifact_id: i64,
        version: i32,
    ) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version WHERE artifact = $1 AND version = $2",
            artifact_version::SELECT_COLUMNS
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(artifact_id)
            .bind(version)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Get a specific readable version by artifact and version number (without binary content).
    pub async fn find_readable_by_version<'e, E>(
        executor: E,
        artifact_id: i64,
        version: i32,
        read_ctx: &ArtifactReadContext,
    ) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT {} FROM artifact_version av JOIN artifact a ON a.id = av.artifact WHERE av.artifact = ",
            Self::select_columns_with_alias("av")
        ));
        query.push_bind(artifact_id);
        query.push(" AND av.version = ");
        query.push_bind(version);
        query.push(" AND ");
        push_artifact_version_read_predicate(&mut query, read_ctx, "a", "av");
        query.push(" LIMIT 1");

        query
            .build_query_as::<ArtifactVersion>()
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Get a specific version by artifact and version number (with binary content)
    pub async fn find_by_version_with_content<'e, E>(
        executor: E,
        artifact_id: i64,
        version: i32,
    ) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version WHERE artifact = $1 AND version = $2",
            artifact_version::SELECT_COLUMNS_WITH_CONTENT
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(artifact_id)
            .bind(version)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Create a new artifact version. The version number is auto-assigned
    /// (MAX(version) + 1) and the retention trigger fires after insert.
    pub async fn create<'e, E>(
        executor: E,
        input: CreateArtifactVersionInput,
    ) -> Result<ArtifactVersion>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if let Some(file_path) = input.file_path.as_deref() {
            ValidatedRelativePath::new(file_path)?;
        }
        let size_bytes = input.content.as_ref().map(|c| c.len() as i64).or_else(|| {
            input
                .content_json
                .as_ref()
                .map(|j| serde_json::to_string(j).unwrap_or_default().len() as i64)
        });

        let query = format!(
            "WITH artifact_lock AS ( \
                 SELECT pg_advisory_xact_lock($1) \
             ), next_version AS ( \
                 SELECT COALESCE(MAX(version), 0) + 1 AS version \
                 FROM artifact_version, artifact_lock \
                 WHERE artifact = $1 \
             ) \
             INSERT INTO artifact_version \
                 (artifact, version, execution, content_type, size_bytes, content, content_json, file_path, meta, created_by) \
             SELECT $1, next_version.version, $2, $3, $4, $5, $6, $7, $8, $9 \
             FROM next_version \
             RETURNING {}",
            artifact_version::SELECT_COLUMNS_WITH_CONTENT
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(input.artifact)
            .bind(input.execution)
            .bind(&input.content_type)
            .bind(size_bytes)
            .bind(&input.content)
            .bind(&input.content_json)
            .bind(&input.file_path)
            .bind(&input.meta)
            .bind(&input.created_by)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    /// Update the `file_path` of a file-backed version after allocation.
    pub async fn update_file_path<'e, E>(
        executor: E,
        version_id: i64,
        file_path: &str,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        ValidatedRelativePath::new(file_path)?;
        let result = sqlx::query("UPDATE artifact_version SET file_path = $1 WHERE id = $2")
            .bind(file_path)
            .bind(version_id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Find a file-backed version for a specific (artifact, execution) pair.
    /// Used to look up the version emitted by a particular execution.
    pub async fn find_by_artifact_and_execution<'e, E>(
        executor: E,
        artifact_id: i64,
        execution_id: i64,
    ) -> Result<Option<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version \
             WHERE artifact = $1 AND execution = $2 \
             ORDER BY version DESC LIMIT 1",
            artifact_version::SELECT_COLUMNS
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(artifact_id)
            .bind(execution_id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Create a file-backed version and populate its computed relative file path.
    pub async fn create_file_backed<'e, E>(
        executor: E,
        artifact_id: i64,
        artifact_ref: &str,
        content_type: String,
        execution: Option<i64>,
        meta: Option<serde_json::Value>,
        created_by: Option<String>,
    ) -> Result<ArtifactVersion>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        validate_artifact_ref(artifact_ref)?;
        let input = CreateArtifactVersionInput {
            artifact: artifact_id,
            execution,
            content_type: Some(content_type.clone()),
            content: None,
            content_json: None,
            file_path: None,
            meta,
            created_by,
        };

        let mut version = loop {
            match Self::create(executor, input.clone()).await {
                Ok(version) => break version,
                Err(crate::error::Error::Database(sqlx::Error::Database(db_err)))
                    if db_err.code().as_deref() == Some("23505")
                        && db_err.constraint().is_some_and(|constraint| {
                            constraint == "uq_artifact_version_artifact_version"
                        }) =>
                {
                    continue;
                }
                Err(err) => return Err(err),
            }
        };
        let file_path = compute_file_path(artifact_ref, version.version, &content_type)?;
        Self::update_file_path(executor, version.id, &file_path).await?;
        version.file_path = Some(file_path);
        Ok(version)
    }

    /// Delete a specific version by ID
    pub async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM artifact_version WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete all versions for an artifact
    pub async fn delete_all_for_artifact<'e, E>(executor: E, artifact_id: i64) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM artifact_version WHERE artifact = $1")
            .bind(artifact_id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected())
    }

    /// Count versions for an artifact
    pub async fn count_by_artifact<'e, E>(executor: E, artifact_id: i64) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM artifact_version WHERE artifact = $1")
            .bind(artifact_id)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }

    /// Update the size_bytes of a specific artifact version (used by worker finalization).
    pub async fn update_size_bytes<'e, E>(
        executor: E,
        version_id: i64,
        size_bytes: i64,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("UPDATE artifact_version SET size_bytes = $1 WHERE id = $2")
            .bind(size_bytes)
            .bind(version_id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Find all file-backed versions linked to an execution.
    /// Filters `artifact_version` by the per-version `execution` column.
    pub async fn find_file_versions_by_execution<'e, E>(
        executor: E,
        execution_id: i64,
    ) -> Result<Vec<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} \
             FROM artifact_version av \
             WHERE av.file_path IS NOT NULL \
               AND av.execution = $1",
            Self::select_columns_with_alias("av")
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(execution_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Find all file-backed versions for a specific artifact (used for disk cleanup on delete).
    pub async fn find_file_versions_by_artifact<'e, E>(
        executor: E,
        artifact_id: i64,
    ) -> Result<Vec<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM artifact_version WHERE artifact = $1 AND file_path IS NOT NULL",
            artifact_version::SELECT_COLUMNS
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(artifact_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Find all file-backed versions for artifacts owned by a specific scope/owner.
    /// Used by standalone sensor agents to copy locally staged sensor artifacts
    /// back to the API-accessible artifact transport when a sensor process exits.
    pub async fn find_file_versions_by_scope_and_owner<'e, E>(
        executor: E,
        scope: OwnerType,
        owner: &str,
    ) -> Result<Vec<ArtifactVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} \
             FROM artifact_version av \
             JOIN artifact a ON a.id = av.artifact \
             WHERE av.file_path IS NOT NULL \
               AND a.scope = $1 \
               AND a.owner = $2",
            Self::select_columns_with_alias("av")
        );
        sqlx::query_as::<_, ArtifactVersion>(&query)
            .bind(scope)
            .bind(owner)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Whether an exact allocated path is owned only by the given execution.
    ///
    /// The negative check matters for upgrade compatibility: historical rows
    /// may contain duplicate paths, and one execution must not mutate bytes
    /// that are also referenced by another execution.
    pub async fn file_path_owned_by_execution<'e, E>(
        executor: E,
        file_path: &str,
        execution_id: i64,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        ValidatedRelativePath::new(file_path)?;
        sqlx::query_scalar(
            "SELECT \
                EXISTS (SELECT 1 FROM artifact_version WHERE file_path = $1 AND execution = $2) \
                AND NOT EXISTS (\
                    SELECT 1 FROM artifact_version \
                    WHERE file_path = $1 AND execution IS DISTINCT FROM $2\
                )",
        )
        .bind(file_path)
        .bind(execution_id)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }

    /// Whether an exact allocated path is owned only by the given sensor.
    pub async fn file_path_owned_by_sensor<'e, E>(
        executor: E,
        file_path: &str,
        sensor_ref: &str,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        ValidatedRelativePath::new(file_path)?;
        sqlx::query_scalar(
            "SELECT \
                EXISTS (\
                    SELECT 1 FROM artifact_version av \
                    JOIN artifact a ON a.id = av.artifact \
                    WHERE av.file_path = $1 AND a.scope = 'sensor' AND a.owner = $2\
                ) \
                AND NOT EXISTS (\
                    SELECT 1 FROM artifact_version av \
                    JOIN artifact a ON a.id = av.artifact \
                    WHERE av.file_path = $1 \
                      AND (a.scope <> 'sensor' OR a.owner IS DISTINCT FROM $2)\
                )",
        )
        .bind(file_path)
        .bind(sensor_ref)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
    }

    /// Whether every version referencing a path is readable under the normal
    /// artifact/version authorization contract. Requiring all aliases to be
    /// readable prevents ambiguous historical duplicate paths from leaking.
    pub async fn file_path_is_readable<'e, E>(
        executor: E,
        file_path: &str,
        read_ctx: &ArtifactReadContext,
    ) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        ValidatedRelativePath::new(file_path)?;
        let mut query = QueryBuilder::<Postgres>::new(
            "SELECT EXISTS (SELECT 1 FROM artifact_version av JOIN artifact a ON a.id = av.artifact WHERE av.file_path = ",
        );
        query.push_bind(file_path);
        query.push(" AND ");
        push_artifact_version_read_predicate(&mut query, read_ctx, "a", "av");
        query.push(") AND NOT EXISTS (SELECT 1 FROM artifact_version av JOIN artifact a ON a.id = av.artifact WHERE av.file_path = ");
        query.push_bind(file_path);
        query.push(" AND NOT ");
        push_artifact_version_read_predicate(&mut query, read_ctx, "a", "av");
        query.push(")");

        query
            .build_query_scalar::<bool>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_artifact, compute_file_path, default_content_type_for_artifact,
        is_file_backed_type, is_runtime_log_artifact_ref, ref_to_dir_path, validate_artifact_ref,
        ArtifactVersionRepository,
    };
    use crate::models::enums::{ArtifactClassification, ArtifactType};

    #[test]
    fn aliased_select_columns_keep_null_content_expression_unqualified() {
        let columns = ArtifactVersionRepository::select_columns_with_alias("av");

        assert!(columns.contains("av.id"));
        assert!(columns.contains("av.file_path"));
        assert!(columns.contains("NULL::bytea AS content"));
        assert!(!columns.contains("av.NULL::bytea AS content"));
    }

    #[test]
    fn test_compute_file_path() {
        assert_eq!(
            compute_file_path("mypack.build_log", 1, "text/plain").unwrap(),
            "mypack/build_log/v1.txt"
        );
        assert_eq!(
            compute_file_path("mypack.build_log", 3, "application/json").unwrap(),
            "mypack/build_log/v3.json"
        );
        assert_eq!(
            compute_file_path("core.test.results", 2, "text/csv").unwrap(),
            "core/test/results/v2.csv"
        );
        assert_eq!(
            compute_file_path("simple", 1, "application/octet-stream").unwrap(),
            "simple/v1.bin"
        );
    }

    #[test]
    fn test_ref_to_dir_path() {
        assert_eq!(
            ref_to_dir_path("mypack.build_log").unwrap(),
            "mypack/build_log"
        );
        assert_eq!(ref_to_dir_path("simple").unwrap(), "simple");
        assert_eq!(ref_to_dir_path("a.b.c.d").unwrap(), "a/b/c/d");
    }

    #[test]
    fn test_validate_artifact_ref_rejects_unsafe_derivations() {
        for artifact_ref in ["", ".hidden", "a..b", "../escape", "a/b", "a\\b", "C:bad"] {
            assert!(
                validate_artifact_ref(artifact_ref).is_err(),
                "{artifact_ref}"
            );
        }
        assert!(validate_artifact_ref("core.echo.stdout.log").is_ok());
    }

    #[test]
    fn test_is_file_backed_type() {
        assert!(is_file_backed_type(ArtifactType::FileBinary));
        assert!(is_file_backed_type(ArtifactType::FileText));
        assert!(is_file_backed_type(ArtifactType::FileDataTable));
        assert!(is_file_backed_type(ArtifactType::FileImage));
        assert!(!is_file_backed_type(ArtifactType::Progress));
        assert!(!is_file_backed_type(ArtifactType::Url));
    }

    #[test]
    fn test_default_content_type_for_artifact() {
        assert_eq!(
            default_content_type_for_artifact(ArtifactType::FileText),
            "text/plain"
        );
        assert_eq!(
            default_content_type_for_artifact(ArtifactType::FileDataTable),
            "text/csv"
        );
        assert_eq!(
            default_content_type_for_artifact(ArtifactType::FileImage),
            "image/png"
        );
        assert_eq!(
            default_content_type_for_artifact(ArtifactType::FileBinary),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_is_runtime_log_artifact_ref() {
        assert!(is_runtime_log_artifact_ref("core.echo.stdout.log"));
        assert!(is_runtime_log_artifact_ref("core.echo.stderr.log"));
        assert!(is_runtime_log_artifact_ref("sensor.core.timer.stdout"));
        assert!(is_runtime_log_artifact_ref("sensor.core.timer.stderr"));
        assert!(!is_runtime_log_artifact_ref("core.workflow.log"));
        assert!(!is_runtime_log_artifact_ref("mypack.build_log"));
    }

    #[test]
    fn test_classify_artifact_marks_runtime_logs() {
        assert_eq!(
            classify_artifact("core.echo.stdout.log", ArtifactType::FileText),
            ArtifactClassification::RuntimeLog
        );
        assert_eq!(
            classify_artifact("sensor.core.timer.stdout", ArtifactType::FileText),
            ArtifactClassification::RuntimeLog
        );
        assert_eq!(
            classify_artifact("core.workflow.log", ArtifactType::FileText),
            ArtifactClassification::General
        );
        assert_eq!(
            classify_artifact("core.echo.stdout.log", ArtifactType::Progress),
            ArtifactClassification::General
        );
    }
}
