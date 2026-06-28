//! Dashboard repositories for metadata and immutable revision history.

use sqlx::{Executor, Postgres, QueryBuilder};

use crate::dashboard_spec::validate_dashboard_spec;
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

#[derive(Debug, Clone)]
struct ResolvedDashboardUpdate {
    scope_type: DashboardScopeType,
    scope_ref: String,
    pack: Option<Id>,
    owner_identity: Option<Id>,
    visibility: DashboardVisibility,
    is_adhoc: bool,
    label: String,
    description: Option<String>,
    enabled: bool,
    is_default_home: bool,
    spec_version: i32,
    spec: JsonDict,
    tags: Vec<String>,
    expected_revision: Option<i32>,
    updated_by: Option<Id>,
    has_changes: bool,
    records_spec_revision: bool,
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
        validate_dashboard_spec(&input.spec).map_err(Error::validation)?;

        let query = format!(
            "WITH cleared AS ( \
                UPDATE dashboard \
                SET is_default_home = FALSE, revision = revision + 1, updated = NOW() \
                WHERE $11 = TRUE \
                 AND scope_type = $2 \
                 AND scope_ref = $3 \
                 AND is_default_home = TRUE \
             ), inserted AS ( \
                INSERT INTO dashboard ( \
                    ref, scope_type, scope_ref, pack, owner_identity, visibility, is_adhoc, \
                    label, description, enabled, is_default_home, revision, spec_version, spec, tags \
                ) VALUES ( \
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 1, $12, $13, $14 \
                ) \
                RETURNING {} \
             ), versioned AS ( \
                INSERT INTO dashboard_version (dashboard, revision, spec_version, spec, created_by) \
                SELECT id, revision, spec_version, spec, $15 FROM inserted \
             ) \
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
        if let Some(spec) = &input.spec {
            validate_dashboard_spec(spec).map_err(Error::validation)?;
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
    fn resolve_update(
        current: &Dashboard,
        input: UpdateDashboardInput,
    ) -> Result<ResolvedDashboardUpdate> {
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

        let UpdateDashboardInput {
            scope_type,
            scope_ref,
            pack,
            owner_identity,
            visibility,
            is_adhoc,
            label,
            description,
            enabled,
            is_default_home,
            spec_version,
            spec,
            tags,
            expected_revision,
            updated_by,
        } = input;

        let validates_spec = spec.is_some() || spec_version.is_some();

        let scope_type = scope_type.unwrap_or(current.scope_type);
        let scope_ref = scope_ref.unwrap_or_else(|| current.scope_ref.clone());
        let pack = match pack {
            Some(Patch::Set(value)) => Some(value),
            Some(Patch::Clear) => None,
            None => current.pack,
        };
        let owner_identity = match owner_identity {
            Some(Patch::Set(value)) => Some(value),
            Some(Patch::Clear) => None,
            None => current.owner_identity,
        };
        let visibility = visibility.unwrap_or(current.visibility);
        let is_adhoc = is_adhoc.unwrap_or(current.is_adhoc);
        let label = label.unwrap_or_else(|| current.label.clone());
        let description = match description {
            Some(Patch::Set(value)) => Some(value),
            Some(Patch::Clear) => None,
            None => current.description.clone(),
        };
        let enabled = enabled.unwrap_or(current.enabled);
        let is_default_home = is_default_home.unwrap_or(current.is_default_home);
        let spec_version = spec_version.unwrap_or(current.spec_version);
        let spec = spec.unwrap_or_else(|| current.spec.clone());
        let tags = tags.unwrap_or_else(|| current.tags.clone());

        if validates_spec && spec_version <= 0 {
            return Err(Error::validation(
                "Dashboard spec_version must be greater than zero",
            ));
        }
        if validates_spec {
            validate_dashboard_spec(&spec).map_err(Error::validation)?;
        }

        let has_changes = scope_type != current.scope_type
            || scope_ref != current.scope_ref
            || pack != current.pack
            || owner_identity != current.owner_identity
            || visibility != current.visibility
            || is_adhoc != current.is_adhoc
            || label != current.label
            || description != current.description
            || enabled != current.enabled
            || is_default_home != current.is_default_home
            || spec_version != current.spec_version
            || spec != current.spec
            || tags != current.tags;

        let records_spec_revision = spec_version != current.spec_version || spec != current.spec;

        Ok(ResolvedDashboardUpdate {
            scope_type,
            scope_ref,
            pack,
            owner_identity,
            visibility,
            is_adhoc,
            label,
            description,
            enabled,
            is_default_home,
            spec_version,
            spec,
            tags,
            expected_revision,
            updated_by,
            has_changes,
            records_spec_revision,
        })
    }

    async fn apply_resolved_update<'e, E>(
        executor: E,
        id: Id,
        resolved: &ResolvedDashboardUpdate,
        record_version: bool,
    ) -> Result<Dashboard>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "WITH cleared AS ( \
                UPDATE dashboard \
                SET is_default_home = FALSE, revision = revision + 1, updated = NOW() \
                WHERE $1 = TRUE \
                  AND scope_type = $2 \
                  AND scope_ref = $3 \
                  AND is_default_home = TRUE \
                  AND id != $4 \
             ), updated AS ( \
                UPDATE dashboard \
                SET scope_type = $2, \
                    scope_ref = $3, \
                    pack = $5, \
                    owner_identity = $6, \
                    visibility = $7, \
                    is_adhoc = $8, \
                    label = $9, \
                    description = $10, \
                    enabled = $11, \
                    is_default_home = $1, \
                    spec_version = $12, \
                    spec = $13, \
                    tags = $14, \
                    revision = revision + 1, \
                    updated = NOW() \
                WHERE id = $4 \
                  AND ($15::INTEGER IS NULL OR revision = $15) \
                RETURNING {} \
             ), versioned AS ( \
                INSERT INTO dashboard_version (dashboard, revision, spec_version, spec, created_by) \
                SELECT id, revision, spec_version, spec, $16 \
                FROM updated \
                WHERE $17 = TRUE \
             ) \
             SELECT {} FROM updated",
            DASHBOARD_SELECT_COLUMNS, DASHBOARD_SELECT_COLUMNS
        );

        let updated = sqlx::query_as::<_, Dashboard>(&query)
            .bind(resolved.is_default_home)
            .bind(resolved.scope_type)
            .bind(&resolved.scope_ref)
            .bind(id)
            .bind(resolved.pack)
            .bind(resolved.owner_identity)
            .bind(resolved.visibility)
            .bind(resolved.is_adhoc)
            .bind(&resolved.label)
            .bind(&resolved.description)
            .bind(resolved.enabled)
            .bind(resolved.spec_version)
            .bind(&resolved.spec)
            .bind(&resolved.tags)
            .bind(resolved.expected_revision)
            .bind(resolved.updated_by)
            .bind(record_version && resolved.records_spec_revision)
            .fetch_optional(executor)
            .await?;

        updated.ok_or_else(|| {
            Error::invalid_state(
                "Dashboard update failed: dashboard not found or revision mismatch",
            )
        })
    }

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
        let current = Self::get_by_id(executor, id).await?;
        let resolved = Self::resolve_update(&current, input)?;
        if !resolved.has_changes {
            return Ok(current);
        }
        Self::apply_resolved_update(executor, id, &resolved, true).await
    }

    pub async fn set_default_home<'e, E>(
        executor: E,
        id: Id,
        expected_revision: Option<i32>,
        updated_by: Option<Id>,
    ) -> Result<Dashboard>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        Self::update_with_version(
            executor,
            id,
            UpdateDashboardInput {
                is_default_home: Some(true),
                expected_revision,
                updated_by,
                ..Default::default()
            },
        )
        .await
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
    use chrono::Utc;
    use serde_json::json;

    fn sample_dashboard() -> Dashboard {
        Dashboard {
            id: 1,
            r#ref: "core.ops".to_string(),
            scope_type: DashboardScopeType::Pack,
            scope_ref: "core".to_string(),
            pack: None,
            owner_identity: None,
            visibility: DashboardVisibility::Pack,
            is_adhoc: false,
            label: "Ops".to_string(),
            description: Some("Operations".to_string()),
            enabled: true,
            is_default_home: false,
            revision: 3,
            spec_version: 1,
            spec: json!({
                "layout": {
                    "breakpoints": {
                        "lg": { "min_width": 1280, "columns": 12 },
                        "sm": { "min_width": 0, "columns": 4 }
                    }
                },
                "data_sources": {
                    "events": { "type": "event_count" }
                },
                "cards": [
                    {
                        "id": "events",
                        "source": "events",
                        "position": {
                            "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                            "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                        }
                    }
                ]
            }),
            tags: vec!["ops".to_string()],
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

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

    #[test]
    fn resolve_update_treats_metadata_only_changes_as_non_versioned() {
        let current = sample_dashboard();
        let resolved = DashboardRepository::resolve_update(
            &current,
            UpdateDashboardInput {
                label: Some("Updated Ops".to_string()),
                updated_by: Some(7),
                ..Default::default()
            },
        )
        .expect("update should resolve");

        assert!(resolved.has_changes);
        assert!(!resolved.records_spec_revision);
        assert_eq!(resolved.label, "Updated Ops");
        assert_eq!(resolved.updated_by, Some(7));
    }

    #[test]
    fn resolve_update_marks_spec_changes_for_revision_history() {
        let current = sample_dashboard();
        let mut new_spec = current.spec.clone();
        new_spec["cards"][0]["title"] = json!("Event Volume");

        let resolved = DashboardRepository::resolve_update(
            &current,
            UpdateDashboardInput {
                spec: Some(new_spec),
                updated_by: Some(9),
                ..Default::default()
            },
        )
        .expect("update should resolve");

        assert!(resolved.has_changes);
        assert!(resolved.records_spec_revision);
        assert_eq!(resolved.updated_by, Some(9));
    }

    #[test]
    fn resolve_update_rejects_invalid_spec_changes() {
        let current = sample_dashboard();
        let mut invalid_spec = current.spec.clone();
        invalid_spec["cards"][0]["position"]["lg"]["w"] = json!(99);

        let err = DashboardRepository::resolve_update(
            &current,
            UpdateDashboardInput {
                spec: Some(invalid_spec),
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(
            matches!(err, Error::Validation(message) if message.contains("width 99 exceeds breakpoint columns 12"))
        );
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
