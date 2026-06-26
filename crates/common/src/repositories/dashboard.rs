//! Dashboard repositories for metadata and immutable revision history.

use sqlx::{Executor, Postgres, QueryBuilder};

use crate::models::{
    dashboard::{
        Dashboard, DashboardVersion, DASHBOARD_SELECT_COLUMNS, DASHBOARD_VERSION_SELECT_COLUMNS,
    },
    DashboardScopeType, DashboardVisibility, Id, JsonDict,
};
use crate::schema::RefValidator;
use crate::{Error, Result};

use super::{Create, Delete, FindById, List, Patch, Repository, Update};

pub struct DashboardRepository;

impl Repository for DashboardRepository {
    type Entity = Dashboard;

    fn table_name() -> &'static str {
        "dashboard"
    }
}

#[derive(Debug, Clone)]
pub struct CreateDashboardInput {
    pub r#ref: String,
    pub scope_type: DashboardScopeType,
    pub scope_ref: String,
    pub pack: Option<Id>,
    pub owner_identity: Option<Id>,
    pub visibility: DashboardVisibility,
    pub is_adhoc: bool,
    pub label: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub is_default_home: bool,
    pub spec_version: i32,
    pub spec: JsonDict,
    pub tags: Vec<String>,
    pub created_by: Option<Id>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateDashboardInput {
    pub scope_type: Option<DashboardScopeType>,
    pub scope_ref: Option<String>,
    pub pack: Option<Patch<Id>>,
    pub owner_identity: Option<Patch<Id>>,
    pub visibility: Option<DashboardVisibility>,
    pub is_adhoc: Option<bool>,
    pub label: Option<String>,
    pub description: Option<Patch<String>>,
    pub enabled: Option<bool>,
    pub is_default_home: Option<bool>,
    pub spec_version: Option<i32>,
    pub spec: Option<JsonDict>,
    pub tags: Option<Vec<String>>,
    pub expected_revision: Option<i32>,
    pub updated_by: Option<Id>,
}

#[derive(Debug, Clone)]
pub struct DashboardScopedRef {
    pub scope_type: DashboardScopeType,
    pub scope_ref: String,
    pub r#ref: String,
}

#[async_trait::async_trait]
impl FindById for DashboardRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM dashboard WHERE id = $1",
            DASHBOARD_SELECT_COLUMNS
        );
        sqlx::query_as::<_, Dashboard>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for DashboardRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM dashboard ORDER BY scope_type ASC, scope_ref ASC, ref ASC LIMIT 1000",
            DASHBOARD_SELECT_COLUMNS
        );
        sqlx::query_as::<_, Dashboard>(&query)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for DashboardRepository {
    type CreateInput = CreateDashboardInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        RefValidator::validate_component_ref(&input.r#ref)?;
        if input.scope_ref.trim().is_empty() {
            return Err(Error::validation("Dashboard scope_ref cannot be empty"));
        }
        if input.spec_version <= 0 {
            return Err(Error::validation(
                "Dashboard spec_version must be greater than zero",
            ));
        }

        let query = format!(
            "WITH inserted AS (\
                 INSERT INTO dashboard (\
                     ref, scope_type, scope_ref, pack, owner_identity, visibility, is_adhoc,\
                     label, description, enabled, is_default_home, revision, spec_version, spec, tags\
                 ) VALUES (\
                     $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 1, $12, $13, $14\
                 )\
                 RETURNING {}\
             ), versioned AS (\
                 INSERT INTO dashboard_version (dashboard, revision, spec_version, spec, created_by)\
                 SELECT id, revision, spec_version, spec, $15 FROM inserted\
             )\
             SELECT {} FROM inserted",
            DASHBOARD_SELECT_COLUMNS, DASHBOARD_SELECT_COLUMNS
        );

        sqlx::query_as::<_, Dashboard>(&query)
            .bind(&input.r#ref)
            .bind(input.scope_type)
            .bind(&input.scope_ref)
            .bind(input.pack)
            .bind(input.owner_identity)
            .bind(input.visibility)
            .bind(input.is_adhoc)
            .bind(&input.label)
            .bind(&input.description)
            .bind(input.enabled)
            .bind(input.is_default_home)
            .bind(input.spec_version)
            .bind(&input.spec)
            .bind(&input.tags)
            .bind(input.created_by)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for DashboardRepository {
    type UpdateInput = UpdateDashboardInput;

    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        if let Some(scope_ref) = &input.scope_ref {
            if scope_ref.trim().is_empty() {
                return Err(Error::validation("Dashboard scope_ref cannot be empty"));
            }
        }
        if let Some(spec_version) = input.spec_version {
            if spec_version <= 0 {
                return Err(Error::validation(
                    "Dashboard spec_version must be greater than zero",
                ));
            }
        }

        let mut query = QueryBuilder::<Postgres>::new("UPDATE dashboard SET ");
        let mut has_updates = false;

        macro_rules! push_comma {
            () => {
                if has_updates {
                    query.push(", ");
                }
            };
        }

        if let Some(scope_type) = input.scope_type {
            push_comma!();
            query.push("scope_type = ").push_bind(scope_type);
            has_updates = true;
        }
        if let Some(scope_ref) = &input.scope_ref {
            push_comma!();
            query.push("scope_ref = ").push_bind(scope_ref);
            has_updates = true;
        }
        if let Some(pack_patch) = &input.pack {
            push_comma!();
            query.push("pack = ");
            match pack_patch {
                Patch::Set(value) => {
                    query.push_bind(value);
                }
                Patch::Clear => {
                    query.push("NULL");
                }
            }
            has_updates = true;
        }
        if let Some(owner_patch) = &input.owner_identity {
            push_comma!();
            query.push("owner_identity = ");
            match owner_patch {
                Patch::Set(value) => {
                    query.push_bind(value);
                }
                Patch::Clear => {
                    query.push("NULL");
                }
            }
            has_updates = true;
        }
        if let Some(visibility) = input.visibility {
            push_comma!();
            query.push("visibility = ").push_bind(visibility);
            has_updates = true;
        }
        if let Some(is_adhoc) = input.is_adhoc {
            push_comma!();
            query.push("is_adhoc = ").push_bind(is_adhoc);
            has_updates = true;
        }
        if let Some(label) = &input.label {
            push_comma!();
            query.push("label = ").push_bind(label);
            has_updates = true;
        }
        if let Some(description_patch) = &input.description {
            push_comma!();
            query.push("description = ");
            match description_patch {
                Patch::Set(value) => {
                    query.push_bind(value);
                }
                Patch::Clear => {
                    query.push("NULL");
                }
            }
            has_updates = true;
        }
        if let Some(enabled) = input.enabled {
            push_comma!();
            query.push("enabled = ").push_bind(enabled);
            has_updates = true;
        }
        if let Some(is_default_home) = input.is_default_home {
            push_comma!();
            query.push("is_default_home = ").push_bind(is_default_home);
            has_updates = true;
        }
        if let Some(spec_version) = input.spec_version {
            push_comma!();
            query.push("spec_version = ").push_bind(spec_version);
            has_updates = true;
        }
        if let Some(spec) = &input.spec {
            push_comma!();
            query.push("spec = ").push_bind(spec);
            has_updates = true;
        }
        if let Some(tags) = &input.tags {
            push_comma!();
            query.push("tags = ").push_bind(tags);
            has_updates = true;
        }

        if !has_updates {
            return Self::get_by_id(executor, id).await;
        }

        query
            .push(", revision = revision + 1, updated = NOW() WHERE id = ")
            .push_bind(id);

        if let Some(expected_revision) = input.expected_revision {
            query.push(" AND revision = ").push_bind(expected_revision);
        }

        query.push(" RETURNING ").push(DASHBOARD_SELECT_COLUMNS);

        let updated = query
            .build_query_as::<Dashboard>()
            .fetch_optional(executor)
            .await?;

        updated.ok_or_else(|| {
            Error::invalid_state(
                "Dashboard update failed: dashboard not found or revision mismatch",
            )
        })
    }
}

#[async_trait::async_trait]
impl Delete for DashboardRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM dashboard WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl DashboardRepository {
    fn visible_scope_precedence(
        dashboard_ref: &str,
        identity_id: Option<Id>,
    ) -> Vec<DashboardScopedRef> {
        let mut scopes = Vec::with_capacity(3);
        if let Some(identity_id) = identity_id {
            scopes.push(DashboardScopedRef {
                scope_type: DashboardScopeType::Identity,
                scope_ref: identity_id.to_string(),
                r#ref: dashboard_ref.to_string(),
            });
        }

        if let Some((pack_ref, _)) = dashboard_ref.split_once('.') {
            scopes.push(DashboardScopedRef {
                scope_type: DashboardScopeType::Pack,
                scope_ref: pack_ref.to_string(),
                r#ref: dashboard_ref.to_string(),
            });
        }

        scopes.push(DashboardScopedRef {
            scope_type: DashboardScopeType::Global,
            scope_ref: "global".to_string(),
            r#ref: dashboard_ref.to_string(),
        });

        scopes
    }

    pub async fn delete_non_adhoc_by_pack_excluding<'e, E>(
        executor: E,
        pack_id: Id,
        keep_refs: &[String],
    ) -> Result<i64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rows = if keep_refs.is_empty() {
            sqlx::query("DELETE FROM dashboard WHERE pack = $1 AND is_adhoc = FALSE")
                .bind(pack_id)
                .execute(executor)
                .await?
        } else {
            sqlx::query(
                "DELETE FROM dashboard \
                 WHERE pack = $1 AND is_adhoc = FALSE AND ref != ALL($2)",
            )
            .bind(pack_id)
            .bind(keep_refs)
            .execute(executor)
            .await?
        };
        Ok(rows.rows_affected() as i64)
    }

    pub async fn update_with_version<'e, E>(
        executor: E,
        id: i64,
        input: UpdateDashboardInput,
    ) -> Result<Dashboard>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let updated_by = input.updated_by;
        let dashboard = <Self as Update>::update(executor, id, input).await?;
        DashboardVersionRepository::create(
            executor,
            CreateDashboardVersionInput {
                dashboard: dashboard.id,
                revision: dashboard.revision,
                spec_version: dashboard.spec_version,
                spec: dashboard.spec.clone(),
                created_by: updated_by,
            },
        )
        .await?;
        Ok(dashboard)
    }

    pub async fn find_by_ref_in_scope<'e, E>(
        executor: E,
        scoped_ref: &DashboardScopedRef,
    ) -> Result<Option<Dashboard>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM dashboard WHERE ref = $1 AND scope_type = $2 AND scope_ref = $3",
            DASHBOARD_SELECT_COLUMNS
        );
        sqlx::query_as::<_, Dashboard>(&query)
            .bind(&scoped_ref.r#ref)
            .bind(scoped_ref.scope_type)
            .bind(&scoped_ref.scope_ref)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn list_by_scope<'e, E>(
        executor: E,
        scope_type: DashboardScopeType,
        scope_ref: &str,
    ) -> Result<Vec<Dashboard>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM dashboard WHERE scope_type = $1 AND scope_ref = $2 ORDER BY ref ASC",
            DASHBOARD_SELECT_COLUMNS
        );
        sqlx::query_as::<_, Dashboard>(&query)
            .bind(scope_type)
            .bind(scope_ref)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_default_home_in_scope<'e, E>(
        executor: E,
        scope_type: DashboardScopeType,
        scope_ref: &str,
    ) -> Result<Option<Dashboard>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM dashboard WHERE scope_type = $1 AND scope_ref = $2 AND is_default_home = TRUE",
            DASHBOARD_SELECT_COLUMNS
        );
        sqlx::query_as::<_, Dashboard>(&query)
            .bind(scope_type)
            .bind(scope_ref)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Resolve a dashboard by ref across the caller-visible scope hierarchy.
    ///
    /// Precedence is: identity scope -> pack scope -> global scope.
    pub async fn find_visible_by_ref<'e, E>(
        executor: E,
        dashboard_ref: &str,
        identity_id: Option<Id>,
    ) -> Result<Option<Dashboard>>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        for scoped_ref in Self::visible_scope_precedence(dashboard_ref, identity_id) {
            if let Some(dashboard) = Self::find_by_ref_in_scope(executor, &scoped_ref).await? {
                return Ok(Some(dashboard));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_scope_precedence_includes_pack_between_identity_and_global() {
        let scopes = DashboardRepository::visible_scope_precedence("core.ops", Some(42));
        assert_eq!(scopes.len(), 3);
        assert_eq!(scopes[0].scope_type, DashboardScopeType::Identity);
        assert_eq!(scopes[0].scope_ref, "42");
        assert_eq!(scopes[1].scope_type, DashboardScopeType::Pack);
        assert_eq!(scopes[1].scope_ref, "core");
        assert_eq!(scopes[2].scope_type, DashboardScopeType::Global);
        assert_eq!(scopes[2].scope_ref, "global");
    }

    #[test]
    fn visible_scope_precedence_skips_identity_when_not_available() {
        let scopes = DashboardRepository::visible_scope_precedence("core.ops", None);
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].scope_type, DashboardScopeType::Pack);
        assert_eq!(scopes[0].scope_ref, "core");
        assert_eq!(scopes[1].scope_type, DashboardScopeType::Global);
        assert_eq!(scopes[1].scope_ref, "global");
    }
}

pub struct DashboardVersionRepository;

impl Repository for DashboardVersionRepository {
    type Entity = DashboardVersion;

    fn table_name() -> &'static str {
        "dashboard_version"
    }
}

#[derive(Debug, Clone)]
pub struct CreateDashboardVersionInput {
    pub dashboard: Id,
    pub revision: i32,
    pub spec_version: i32,
    pub spec: JsonDict,
    pub created_by: Option<Id>,
}

#[async_trait::async_trait]
impl FindById for DashboardVersionRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM dashboard_version WHERE id = $1",
            DASHBOARD_VERSION_SELECT_COLUMNS
        );
        sqlx::query_as::<_, DashboardVersion>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for DashboardVersionRepository {
    type CreateInput = CreateDashboardVersionInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "INSERT INTO dashboard_version (dashboard, revision, spec_version, spec, created_by) \
             VALUES ($1, $2, $3, $4, $5) RETURNING {}",
            DASHBOARD_VERSION_SELECT_COLUMNS
        );
        sqlx::query_as::<_, DashboardVersion>(&query)
            .bind(input.dashboard)
            .bind(input.revision)
            .bind(input.spec_version)
            .bind(&input.spec)
            .bind(input.created_by)
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

impl DashboardVersionRepository {
    pub async fn list_by_dashboard<'e, E>(
        executor: E,
        dashboard_id: Id,
    ) -> Result<Vec<DashboardVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM dashboard_version WHERE dashboard = $1 ORDER BY revision DESC",
            DASHBOARD_VERSION_SELECT_COLUMNS
        );
        sqlx::query_as::<_, DashboardVersion>(&query)
            .bind(dashboard_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_dashboard_and_revision<'e, E>(
        executor: E,
        dashboard_id: Id,
        revision: i32,
    ) -> Result<Option<DashboardVersion>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {} FROM dashboard_version WHERE dashboard = $1 AND revision = $2",
            DASHBOARD_VERSION_SELECT_COLUMNS
        );
        sqlx::query_as::<_, DashboardVersion>(&query)
            .bind(dashboard_id)
            .bind(revision)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}
