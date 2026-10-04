//! Atomic platform metadata reconciliation and managed-component ownership reads.

use serde_json::json;
use sqlx::{Executor, PgConnection, PgPool, Postgres};

use crate::models::{IntrinsicHandler, ManagementOrigin, ManagementOriginKind as StoredOrigin};
use crate::platform_catalog::{
    definitions, ManagedComponentKind, CATALOG_REVISION, COMPATIBILITY_EPOCH,
};
use crate::version_matching::extract_version_components;
use crate::{Error, Result};

#[derive(sqlx::FromRow)]
struct OwnershipRow {
    management_origin: StoredOrigin,
    catalog_revision: Option<i32>,
    pack: Option<i64>,
    managed_release: Option<i64>,
}

impl OwnershipRow {
    fn into_origin(self) -> Result<ManagementOrigin> {
        match (self.management_origin, self.catalog_revision, self.pack) {
            (StoredOrigin::Platform, Some(catalog_revision), None) => {
                Ok(ManagementOrigin::Platform { catalog_revision })
            }
            (StoredOrigin::Pack, None, Some(pack_id)) => Ok(ManagementOrigin::Pack {
                pack_id,
                release_id: self.managed_release,
            }),
            (StoredOrigin::AdHoc, None, _) => Ok(ManagementOrigin::AdHoc),
            _ => Err(Error::invalid_state(
                "Invalid persisted component ownership",
            )),
        }
    }
}

pub struct PlatformCatalogRepository;

impl PlatformCatalogRepository {
    pub async fn intrinsic_handler<'e, E>(
        executor: E,
        handler_ref: &str,
    ) -> Result<Option<IntrinsicHandler>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        Ok(sqlx::query_as("SELECT ref, catalog_revision, allowed_component_refs, param_schema, out_schema FROM intrinsic_handler WHERE ref = $1")
            .bind(handler_ref).fetch_optional(executor).await?)
    }

    /// Connection setup also serves the migration command, which must be able to
    /// open a pre-catalog database. API startup requires reconciliation separately.
    pub async fn check_existing_epoch(pool: &PgPool) -> Result<()> {
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('platform_catalog_state') IS NOT NULL")
                .fetch_one(pool)
                .await?;
        if exists {
            Self::check_compatibility(pool).await?;
        }
        Ok(())
    }

    pub async fn check_compatibility<'e, E>(executor: E) -> Result<()>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let (epoch, revision): (i32, i32) = sqlx::query_as(
            "SELECT compatibility_epoch, revision FROM platform_catalog_state WHERE singleton",
        )
        .fetch_one(executor)
        .await?;
        if epoch != COMPATIBILITY_EPOCH || revision > CATALOG_REVISION {
            return Err(Error::invalid_state(format!(
                "Unsupported platform catalog epoch/revision {epoch}/{revision}; binary supports {COMPATIBILITY_EPOCH}/{CATALOG_REVISION}. Use a compatible deployment; do not downgrade the catalog."
            )));
        }
        Ok(())
    }

    pub async fn origin<'e, E>(
        executor: E,
        kind: ManagedComponentKind,
        component_ref: &str,
    ) -> Result<Option<ManagementOrigin>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let owner_column = if kind == ManagedComponentKind::Cache {
            "managing_pack"
        } else {
            "pack"
        };
        let ref_column = if kind == ManagedComponentKind::Cache {
            "definition_ref"
        } else {
            "ref"
        };
        let query = format!(
            "SELECT management_origin, catalog_revision, {owner_column} AS pack, managed_release FROM {} WHERE {ref_column} = $1",
            kind.table()
        );
        let mut rows: Vec<OwnershipRow> = sqlx::query_as(&format!("{query} LIMIT 2"))
            .bind(component_ref)
            .fetch_all(executor)
            .await?;
        if rows.len() > 1 {
            return Err(Error::validation(
                "Scoped component ref is ambiguous; use its ID for ownership lookup",
            ));
        }
        rows.pop().map(OwnershipRow::into_origin).transpose()
    }

    /// ID lookup also supports scoped dashboards and cache definitions.
    pub async fn origin_by_id<'e, E>(
        executor: E,
        kind: ManagedComponentKind,
        id: i64,
    ) -> Result<Option<ManagementOrigin>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let owner = if kind == ManagedComponentKind::Cache {
            "managing_pack"
        } else {
            "pack"
        };
        let row: Option<OwnershipRow> = sqlx::query_as(&format!(
            "SELECT management_origin, catalog_revision, {owner} AS pack, managed_release FROM {} WHERE id = $1", kind.table()
        )).bind(id).fetch_optional(executor).await?;
        row.map(OwnershipRow::into_origin).transpose()
    }

    pub async fn ensure_pack_owner(
        connection: &mut PgConnection,
        kind: ManagedComponentKind,
        component_ref: &str,
        pack_id: i64,
    ) -> Result<()> {
        match Self::origin(connection, kind, component_ref).await? {
            None => Ok(()),
            Some(ManagementOrigin::Pack { pack_id: owner, .. }) if owner == pack_id => Ok(()),
            Some(owner) => Err(Error::validation(format!(
                "Cannot replace {} '{component_ref}': owned by {owner:?}",
                kind.table()
            ))),
        }
    }

    /// Reconcile before bootstrap authorization or pack writes. No installed pack
    /// or filesystem content is needed. Every transfer and update commits together.
    pub async fn reconcile(pool: &PgPool) -> Result<()> {
        let mut tx = pool.begin().await?;
        // A singleton row lock is schema-local, including schema-per-test fixtures.
        let revision: i32 = sqlx::query_scalar(
            "SELECT revision FROM platform_catalog_state WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        Self::check_compatibility(&mut *tx).await?;
        let definitions = definitions()?;
        let mut bootstrap_definitions = json!({});
        for (kind, definition) in &definitions {
            let component_ref = definition["ref"]
                .as_str()
                .ok_or_else(|| Error::internal("Catalog ref missing"))?;
            bootstrap_definitions[kind.table()][component_ref] = definition.clone();
        }
        // Backfill the source snapshot even when metadata is already at this
        // revision. The row lock also serializes the Python bootstrap bridge.
        sqlx::query("UPDATE platform_catalog_state SET bootstrap_definitions = $1 WHERE singleton AND bootstrap_definitions IS DISTINCT FROM $1")
            .bind(bootstrap_definitions).execute(&mut *tx).await?;
        if revision == CATALOG_REVISION {
            tx.commit().await?;
            return Ok(());
        }

        // Block ordinary writers between ownership validation and upsert, including
        // first-insert races for refs that have no row to lock yet.
        sqlx::query("LOCK TABLE permission_set, runtime, trigger, runtime_version, intrinsic_handler IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx).await?;
        for (kind, definition) in &definitions {
            let component_ref = definition["ref"]
                .as_str()
                .ok_or_else(|| Error::internal("Catalog ref missing"))?;
            match Self::origin(&mut *tx, *kind, component_ref).await? {
                None | Some(ManagementOrigin::Platform { .. }) => {}
                Some(ManagementOrigin::Pack { pack_id, .. }) if revision == 0 => {
                    let legacy_core: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM pack WHERE id = $1 AND ref = 'core')",
                    ).bind(pack_id).fetch_one(&mut *tx).await?;
                    let query = format!(
                        "SELECT pack_ref = 'core'{} FROM {} WHERE ref = $1",
                        match kind {
                            ManagedComponentKind::Runtime => " AND NOT auto_detected",
                            ManagedComponentKind::Trigger => " AND sensor IS NULL AND sensor_ref IS NULL",
                            _ => "",
                        }, kind.table()
                    );
                    let expected: Option<bool> = sqlx::query_scalar(&query)
                        .bind(component_ref).fetch_one(&mut *tx).await?;
                    if !legacy_core || expected != Some(true) {
                        return Err(Error::invalid_state(format!("Unexpected legacy owner for {component_ref}; catalog transfer aborted")));
                    }
                    if *kind == ManagedComponentKind::Runtime {
                        let versions = definition["versions"].as_array().into_iter().flatten()
                            .map(|version| version["version"].as_str().ok_or_else(|| Error::internal("Catalog runtime version missing")))
                            .collect::<Result<Vec<_>>>()?;
                        let unexpected: Vec<String> = sqlx::query_scalar(
                            "SELECT v.version FROM runtime_version v JOIN runtime r ON r.id = v.runtime
                             WHERE r.ref = $1 AND (v.version != ALL($2::text[]) OR v.runtime_ref <> $1)
                             ORDER BY v.version",
                        ).bind(component_ref).bind(versions).fetch_all(&mut *tx).await?;
                        if !unexpected.is_empty() {
                            return Err(Error::invalid_state(format!(
                                "Unexpected runtime versions for {component_ref}: {}; catalog transfer aborted without deleting children",
                                unexpected.join(", ")
                            )));
                        }
                    }
                }
                Some(owner) => return Err(Error::invalid_state(format!(
                    "Catalog ownership conflict for {component_ref}: {owner:?}; operator review required"
                ))),
            }
        }

        sqlx::query("SELECT set_config('attune.catalog_write', 'on', true)")
            .execute(&mut *tx)
            .await?;
        for (kind, definition) in definitions {
            match kind {
                ManagedComponentKind::PermissionSet => {
                    sqlx::query(
                        "INSERT INTO permission_set (ref, label, description, grants, catalog_revision)
                         VALUES ($1->>'ref', $1->>'label', $1->>'description', $1->'grants', $2)
                         ON CONFLICT (ref) DO UPDATE SET pack = NULL, pack_ref = NULL,
                         label = EXCLUDED.label, description = EXCLUDED.description,
                         grants = EXCLUDED.grants, catalog_revision = EXCLUDED.catalog_revision",
                    ).bind(&definition).bind(CATALOG_REVISION).execute(&mut *tx).await?;
                }
                ManagedComponentKind::Runtime => {
                    let runtime_id: i64 = sqlx::query_scalar(
                        "INSERT INTO runtime (ref, name, description, aliases, distributions, installation,
                         execution_config, catalog_revision)
                         VALUES ($1->>'ref', $1->>'name', $1->>'description',
                         ARRAY(SELECT jsonb_array_elements_text($1->'aliases')), $1->'distributions',
                         $1->'installation', $1->'execution_config', $2)
                         ON CONFLICT (ref) DO UPDATE SET pack = NULL, pack_ref = NULL,
                         name = EXCLUDED.name, description = EXCLUDED.description, aliases = EXCLUDED.aliases,
                         distributions = EXCLUDED.distributions, installation = EXCLUDED.installation,
                         execution_config = EXCLUDED.execution_config, catalog_revision = EXCLUDED.catalog_revision
                         RETURNING id",
                    ).bind(&definition).bind(CATALOG_REVISION).fetch_one(&mut *tx).await?;
                    if let Some(versions) = definition["versions"].as_array() {
                        for version in versions {
                            let version_string = version["version"].as_str().ok_or_else(|| {
                                Error::internal("Catalog runtime version missing")
                            })?;
                            let (major, minor, patch) = extract_version_components(version_string);
                            sqlx::query(
                                "INSERT INTO runtime_version (runtime, runtime_ref, version, version_major,
                                 version_minor, version_patch, execution_config, distributions, is_default, meta)
                                 VALUES ($1, $2, $3->>'version', $4, $5, $6, $3->'execution_config',
                                 $3->'distributions', COALESCE(($3->>'is_default')::boolean, false), COALESCE($3->'meta', '{}'::jsonb))
                                 ON CONFLICT (runtime, version) DO UPDATE SET
                                 execution_config = EXCLUDED.execution_config, distributions = EXCLUDED.distributions,
                                 is_default = EXCLUDED.is_default, meta = EXCLUDED.meta",
                            ).bind(runtime_id).bind(definition["ref"].as_str()).bind(version)
                                .bind(major).bind(minor).bind(patch).execute(&mut *tx).await?;
                        }
                    }
                }
                ManagedComponentKind::Trigger => {
                    sqlx::query(
                        "INSERT INTO trigger (ref, label, description, enabled, param_schema, out_schema, catalog_revision)
                         VALUES ($1->>'ref', $1->>'label', $1->>'description', true, $1->'parameters', $1->'output', $2)
                         ON CONFLICT (ref) DO UPDATE SET pack = NULL, pack_ref = NULL,
                         label = EXCLUDED.label, description = EXCLUDED.description, enabled = true,
                         param_schema = EXCLUDED.param_schema, out_schema = EXCLUDED.out_schema,
                         catalog_revision = EXCLUDED.catalog_revision",
                    ).bind(&definition).bind(CATALOG_REVISION).execute(&mut *tx).await?;
                }
                _ => unreachable!("only platform metadata belongs in the built-in catalog"),
            }
        }
        sqlx::query("UPDATE platform_catalog_state SET revision = $1 WHERE singleton")
            .bind(CATALOG_REVISION)
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT set_config('attune.catalog_write', 'off', true)")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
