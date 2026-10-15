//! Mutable type projections and immutable release snapshots. All DB access for
//! named schema definitions belongs here; callers authorize dependency edges.

use std::collections::BTreeMap;

use sqlx::{Executor, PgConnection, Postgres, Transaction};

use crate::models::schema_type::{SchemaType, SchemaTypeRevision};
use crate::platform_catalog::ManagedComponentKind;
use crate::schema_type::SchemaTypeDefinition;
use crate::value_schema::{
    Contract, RegistryEntry, RegistryView, SchemaContext, SourceIdentity, TypeRevision,
};
use crate::{Error, Result};

const SELECT_COLUMNS: &str = "id, ref, pack, pack_ref, label, description, schema, reference_visibility, reference_allowed_pack_refs, managed_release, retired_at, created, updated";
const REVISION_COLUMNS: &str = "id, release, type_ref, definition, content_digest, created";

pub struct SchemaTypeRepository;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActiveTypeRevision {
    pub revision_id: i64,
    pub release_id: i64,
    pub release_digest: String,
    pub pack_ref: String,
    pub version: String,
    pub dependencies: serde_json::Value,
    pub definition: serde_json::Value,
    pub content_digest: String,
    pub active: bool,
}

impl SchemaTypeRepository {
    pub async fn find_installed<'e, E>(executor: E, type_ref: &str) -> Result<Option<SchemaType>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let columns = SELECT_COLUMNS
            .split(", ")
            .map(|column| format!("t.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(sqlx::query_as(&format!("SELECT {columns} FROM schema_type t JOIN pack p ON p.id = t.pack WHERE t.ref = $1 AND t.retired_at IS NULL AND p.active_release IS NOT NULL"))
            .bind(type_ref).fetch_optional(executor).await?)
    }

    pub async fn list_installed(
        connection: &mut PgConnection,
        pack_ref: Option<&str>,
        query: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<SchemaType>, u64)> {
        let patterns = super::text_search_patterns(query);
        let condition = "t.retired_at IS NULL AND p.active_release IS NOT NULL AND ($1::TEXT IS NULL OR t.pack_ref = $1) \
            AND NOT EXISTS(SELECT 1 FROM unnest($2::TEXT[]) patterns(pattern) WHERE NOT(t.ref ILIKE pattern OR t.label ILIKE pattern OR COALESCE(t.description, '') ILIKE pattern))";
        let total: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM schema_type t JOIN pack p ON p.id = t.pack WHERE {condition}"
        ))
        .bind(pack_ref)
        .bind(&patterns)
        .fetch_one(&mut *connection)
        .await?;
        let columns = SELECT_COLUMNS
            .split(", ")
            .map(|column| format!("t.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        let rows = sqlx::query_as(&format!("SELECT {columns} FROM schema_type t JOIN pack p ON p.id = t.pack WHERE {condition} ORDER BY t.ref LIMIT $3 OFFSET $4"))
            .bind(pack_ref).bind(&patterns).bind(limit.clamp(1, 100)).bind(offset.max(0)).fetch_all(connection).await?;
        Ok((rows, total as u64))
    }
    /// Creation takes shared graph admission before owner/rule locks. It reads
    /// projections without locking pack rows; registration retains pack-first
    /// exclusive admission, so neither path reverses pack/rule lock order.
    pub async fn lock_selection(connection: &mut PgConnection) -> Result<()> {
        sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended(current_schema() || ':named-type-graph', 0))")
            .execute(connection).await?;
        Ok(())
    }
    /// Reclamation takes exclusive admission before inspecting runtime pins.
    /// It never locks pack rows, so it cannot reverse registration's pack-first
    /// order. New bindings become visible before eligible releases are deleted.
    pub async fn lock_reclamation(connection: &mut PgConnection) -> Result<()> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph', 0))")
            .execute(connection).await?;
        Ok(())
    }
    pub async fn refresh_type_references(connection: &mut PgConnection, pack: i64) -> Result<()> {
        Self::lock_graph(&mut *connection, pack).await?;
        let types = Self::list_by_pack(&mut *connection, pack).await?;
        sqlx::query("DELETE FROM schema_type_reference WHERE source_pack = $1 AND source_kind = 'schema_type'")
            .bind(pack).execute(&mut *connection).await?;
        for definition in types {
            let refs = crate::value_schema::inspect_definition(
                SchemaContext::SingleValue,
                &definition.schema,
                crate::value_schema::Limits::default(),
            )
            .map_err(|error| {
                Error::validation(format!("Invalid named type dependency: {error}"))
            })?;
            let refs: std::collections::BTreeSet<_> = refs
                .into_iter()
                .map(|reference| reference.type_ref)
                .collect();
            for target in refs {
                sqlx::query("INSERT INTO schema_type_reference (source_pack, source_kind, source_ref, schema_field, target_ref) VALUES ($1, 'schema_type', $2, 'schema', $3)")
                    .bind(pack).bind(&definition.r#ref).bind(target).execute(&mut *connection).await?;
            }
        }
        Ok(())
    }

    pub async fn refresh_consumer_references(
        connection: &mut PgConnection,
        pack: i64,
    ) -> Result<()> {
        let sources: Vec<(String, String, String, serde_json::Value)> = sqlx::query_as(
            "SELECT 'pack', ref, 'conf_schema', conf_schema FROM pack WHERE id = $1 \
            UNION ALL SELECT 'action', ref, 'param_schema', param_schema FROM action WHERE pack = $1 AND retired_at IS NULL AND param_schema IS NOT NULL \
            UNION ALL SELECT 'action', ref, 'out_schema', out_schema FROM action WHERE pack = $1 AND retired_at IS NULL AND out_schema IS NOT NULL \
            UNION ALL SELECT 'trigger', ref, 'param_schema', param_schema FROM trigger WHERE pack = $1 AND retired_at IS NULL AND param_schema IS NOT NULL \
            UNION ALL SELECT 'trigger', ref, 'out_schema', out_schema FROM trigger WHERE pack = $1 AND retired_at IS NULL AND out_schema IS NOT NULL \
            UNION ALL SELECT 'trigger', ref, 'event_config_schema', event_config_schema FROM trigger WHERE pack = $1 AND retired_at IS NULL AND event_config_schema IS NOT NULL \
            UNION ALL SELECT 'sensor', ref, 'param_schema', param_schema FROM sensor WHERE pack = $1 AND retired_at IS NULL AND param_schema IS NOT NULL \
             UNION ALL SELECT 'workflow', ref, 'param_schema', COALESCE(param_schema, NULLIF(definition->'parameters', 'null'::jsonb)) FROM workflow_definition WHERE pack = $1 AND retired_at IS NULL AND COALESCE(param_schema, NULLIF(definition->'parameters', 'null'::jsonb)) IS NOT NULL \
             UNION ALL SELECT 'workflow', ref, 'out_schema', COALESCE(out_schema, NULLIF(definition->'output', 'null'::jsonb)) FROM workflow_definition WHERE pack = $1 AND retired_at IS NULL AND COALESCE(out_schema, NULLIF(definition->'output', 'null'::jsonb)) IS NOT NULL \
            UNION ALL SELECT 'queue', ref, 'item_schema', item_schema FROM work_queue WHERE pack = $1 AND retired_at IS NULL"
        ).bind(pack).fetch_all(&mut *connection).await?;
        sqlx::query("DELETE FROM schema_type_reference WHERE source_pack = $1 AND source_kind <> 'schema_type'")
            .bind(pack).execute(&mut *connection).await?;
        for (kind, reference, field, schema) in sources {
            let targets = crate::value_schema::inspect_definition(
                SchemaContext::FlatDocument,
                &schema,
                crate::value_schema::Limits::default(),
            )
            .map_err(|error| {
                Error::validation(format!("Invalid consumer schema references: {error}"))
            })?;
            for target in targets {
                sqlx::query("INSERT INTO schema_type_reference (source_pack,source_kind,source_ref,schema_field,target_ref) \
                    VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                    .bind(pack).bind(&kind).bind(&reference).bind(&field).bind(target.type_ref).execute(&mut *connection).await?;
            }
        }
        Ok(())
    }

    pub async fn protect_omissions(
        connection: &mut PgConnection,
        pack: i64,
        keep: &[i64],
    ) -> Result<()> {
        Self::lock_graph(&mut *connection, pack).await?;
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM schema_type t JOIN schema_type_reference r ON r.target_ref = t.ref \
              WHERE t.pack = $1 AND t.retired_at IS NULL AND NOT(t.id = ANY($2::BIGINT[])) AND r.source_pack IS DISTINCT FROM $1 \
              AND (\
              (r.source_kind = 'schema_type' AND EXISTS(SELECT 1 FROM schema_type source WHERE source.ref = r.source_ref AND source.retired_at IS NULL)) OR \
              (r.source_kind = 'pack' AND EXISTS(SELECT 1 FROM pack source WHERE source.id = r.source_pack AND source.ref = r.source_ref)) OR \
              (r.source_kind = 'action' AND EXISTS(SELECT 1 FROM action source WHERE source.ref = r.source_ref AND source.retired_at IS NULL)) OR \
              (r.source_kind = 'trigger' AND EXISTS(SELECT 1 FROM trigger source WHERE source.ref = r.source_ref AND source.retired_at IS NULL)) OR \
              (r.source_kind = 'sensor' AND EXISTS(SELECT 1 FROM sensor source WHERE source.ref = r.source_ref AND source.retired_at IS NULL)) OR \
              (r.source_kind = 'workflow' AND EXISTS(SELECT 1 FROM workflow_definition source WHERE source.ref = r.source_ref AND source.retired_at IS NULL)) OR \
              (r.source_kind = 'queue' AND EXISTS(SELECT 1 FROM work_queue source WHERE source.ref = r.source_ref AND source.retired_at IS NULL))))"
        ).bind(pack).bind(keep).fetch_one(connection).await?;
        if blocked {
            return Err(Error::validation(
                "Cannot retire a named type referenced by another active consumer",
            ));
        }
        Ok(())
    }
    /// Replace only one component's edges, preserving concurrent edits of other consumers.
    /// The caller holds shared graph admission through its owner mutation.
    pub async fn replace_consumer_references(
        connection: &mut PgConnection,
        pack: impl Into<Option<i64>> + Send,
        kind: &str,
        reference: &str,
        fields: &[(&str, Option<&serde_json::Value>)],
    ) -> Result<()> {
        if !matches!(
            kind,
            "pack" | "action" | "trigger" | "sensor" | "workflow" | "queue"
        ) {
            return Err(Error::validation("Invalid schema consumer kind"));
        }
        let pack = pack.into();
        let mut selected = Vec::with_capacity(fields.len());
        for (field, schema) in fields {
            if !matches!(
                (kind, *field),
                ("pack", "conf_schema")
                    | ("action" | "workflow", "param_schema" | "out_schema")
                    | (
                        "trigger",
                        "param_schema" | "out_schema" | "event_config_schema"
                    )
                    | ("sensor", "param_schema")
                    | ("queue", "item_schema")
            ) {
                return Err(Error::validation("Invalid schema consumer field"));
            }
            let targets = match schema {
                Some(schema) => crate::value_schema::inspect_definition(
                    SchemaContext::FlatDocument,
                    schema,
                    crate::value_schema::Limits::default(),
                )
                .map_err(|error| {
                    Error::validation(format!("Invalid consumer schema references: {error}"))
                })?,
                None => Vec::new(),
            };
            selected.push((field, targets));
        }
        for (field, targets) in selected {
            sqlx::query("DELETE FROM schema_type_reference WHERE source_pack IS NOT DISTINCT FROM $1 AND source_kind = $2 AND source_ref = $3 AND schema_field = $4")
                .bind(pack).bind(kind).bind(reference).bind(field).execute(&mut *connection).await?;
            for target in targets {
                sqlx::query("INSERT INTO schema_type_reference (source_pack,source_kind,source_ref,schema_field,target_ref) VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                    .bind(pack).bind(kind).bind(reference).bind(field).bind(target.type_ref).execute(&mut *connection).await?;
            }
        }
        Ok(())
    }

    /// Removing a consumer and its authoring edges is one statement. Edge
    /// removal only relaxes protection; immutable admitted contracts retain
    /// their own release pins independently of these mutable declarations.
    pub(crate) async fn delete_consumer_row<'e, E>(executor: E, kind: &str, id: i64) -> Result<bool>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres> + 'e,
    {
        let table = match kind {
            "action" => "action",
            "trigger" => "trigger",
            "sensor" => "sensor",
            "workflow" => "workflow_definition",
            _ => return Err(Error::validation("Invalid schema consumer deletion")),
        };
        let count: i64 = sqlx::query_scalar(&format!(
            "WITH deleted AS (DELETE FROM {table} WHERE id = $1 RETURNING pack, ref), \
             removed_edges AS (DELETE FROM schema_type_reference edge USING deleted \
               WHERE edge.source_pack IS NOT DISTINCT FROM deleted.pack AND edge.source_ref = deleted.ref AND edge.source_kind = $2) \
             SELECT COUNT(*) FROM deleted"
        )).bind(id).bind(kind).fetch_one(executor).await?;
        Ok(count > 0)
    }

    /// Pack-first lock order. Type graph writers hold this admission until commit.
    pub async fn lock_graph(connection: &mut PgConnection, pack: i64) -> Result<()> {
        sqlx::query("SELECT id FROM pack WHERE id = $1 FOR NO KEY UPDATE")
            .bind(pack)
            .execute(&mut *connection)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph', 0))")
            .execute(connection).await?;
        Ok(())
    }

    pub async fn active_revision(
        connection: &mut PgConnection,
        type_ref: &str,
    ) -> Result<Option<ActiveTypeRevision>> {
        Ok(sqlx::query_as(
            "SELECT tr.id AS revision_id, pr.id AS release_id, pr.digest AS release_digest, pr.pack_ref, pr.version, \
             COALESCE(pr.manifest->'dependencies', '{}'::jsonb) AS dependencies, tr.definition, tr.content_digest, \
             EXISTS(SELECT 1 FROM schema_type t WHERE t.ref = tr.type_ref AND t.retired_at IS NULL) AS active \
             FROM pack p JOIN pack_release pr ON pr.id = p.active_release \
             JOIN schema_type_revision tr ON tr.release = pr.id AND tr.type_ref = $1 \
             WHERE p.ref = split_part($1, '.', 1)"
        ).bind(type_ref).fetch_optional(connection).await?)
    }
    pub async fn find_by_ref<'e, E>(executor: E, type_ref: &str) -> Result<Option<SchemaType>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        Ok(sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM schema_type WHERE ref = $1"
        ))
        .bind(type_ref)
        .fetch_optional(executor)
        .await?)
    }

    pub async fn list_by_pack<'e, E>(executor: E, pack: i64) -> Result<Vec<SchemaType>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        Ok(sqlx::query_as(&format!("SELECT {SELECT_COLUMNS} FROM schema_type WHERE pack = $1 AND retired_at IS NULL ORDER BY ref"))
            .bind(pack).fetch_all(executor).await?)
    }

    /// A definition must have passed full graph/default/secret compilation before
    /// it enters the projection. Ownership checks cannot be skipped by upsert.
    pub async fn upsert(
        connection: &mut PgConnection,
        pack: i64,
        definition: &SchemaTypeDefinition,
        validated: &Contract,
    ) -> Result<SchemaType> {
        Self::lock_graph(&mut *connection, pack).await?;
        let pack_ref: String =
            sqlx::query_scalar("SELECT ref FROM pack WHERE id = $1 FOR NO KEY UPDATE")
                .bind(pack)
                .fetch_optional(&mut *connection)
                .await?
                .ok_or_else(|| Error::not_found("pack", "id", pack.to_string()))?;
        definition.validate(&pack_ref)?;
        let dependents: Vec<Option<String>> = sqlx::query_scalar(
            "WITH active_sources AS (\
             SELECT 'schema_type' AS kind, ref, pack FROM schema_type WHERE retired_at IS NULL \
             UNION ALL SELECT 'pack', ref, id FROM pack \
             UNION ALL SELECT 'action', ref, pack FROM action WHERE retired_at IS NULL \
             UNION ALL SELECT 'trigger', ref, pack FROM trigger WHERE retired_at IS NULL \
             UNION ALL SELECT 'sensor', ref, pack FROM sensor WHERE retired_at IS NULL \
             UNION ALL SELECT 'workflow', ref, pack FROM workflow_definition WHERE retired_at IS NULL \
             UNION ALL SELECT 'queue', ref, pack FROM work_queue WHERE retired_at IS NULL) \
             SELECT DISTINCT owner.ref FROM schema_type_reference r \
              JOIN active_sources source ON source.ref = r.source_ref AND source.kind = r.source_kind AND source.pack IS NOT DISTINCT FROM r.source_pack \
              LEFT JOIN pack owner ON owner.id = source.pack \
              WHERE r.target_ref = $1 AND r.source_pack IS DISTINCT FROM $2",
        )
        .bind(&definition.r#ref)
        .bind(pack)
        .fetch_all(&mut *connection)
        .await?;
        if dependents.iter().any(|source| match source {
            Some(source) => !definition.reference_allowed(source),
            None => {
                definition.reference_visibility != crate::models::ActionReferenceVisibility::Public
            }
        }) {
            return Err(Error::validation(
                "Cannot restrict a named type while another active consumer references it",
            ));
        }
        crate::value_schema::CompiledContract::new(validated)
            .map_err(|error| Error::validation(format!("Invalid named type contract: {error}")))?;
        let digest = crate::value_schema::digest(&definition.schema)
            .map_err(|error| Error::validation(format!("Invalid named type body: {error}")))?;
        if validated.source.component_ref() != definition.r#ref
            || validated.context != SchemaContext::SingleValue
            || validated.authored_digest != digest
        {
            return Err(Error::validation(
                "Named type contract does not describe this definition",
            ));
        }
        super::platform_catalog::PlatformCatalogRepository::ensure_pack_owner(
            &mut *connection,
            ManagedComponentKind::SchemaType,
            &definition.r#ref,
            pack,
        )
        .await?;
        Ok(sqlx::query_as(&format!(
            "INSERT INTO schema_type (ref, pack, pack_ref, label, description, schema, reference_visibility, reference_allowed_pack_refs) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (ref) DO UPDATE SET label = EXCLUDED.label, description = EXCLUDED.description, \
             schema = EXCLUDED.schema, reference_visibility = EXCLUDED.reference_visibility, \
             reference_allowed_pack_refs = EXCLUDED.reference_allowed_pack_refs, retired_at = NULL, updated = NOW() \
             WHERE schema_type.pack = EXCLUDED.pack RETURNING {SELECT_COLUMNS}"
        )).bind(&definition.r#ref).bind(pack).bind(pack_ref).bind(&definition.label)
            .bind(&definition.description).bind(&definition.schema).bind(definition.reference_visibility)
            .bind(&definition.reference_allowed_pack_refs).fetch_one(connection).await?)
    }

    /// Freeze the complete effective type view, including definitions retained by
    /// omission policy. Re-activating a release cannot change that view silently.
    pub async fn capture_release(
        transaction: &mut Transaction<'_, Postgres>,
        pack: i64,
        release: i64,
    ) -> Result<()> {
        let connection = &mut **transaction;
        Self::lock_graph(&mut *connection, pack).await?;
        // Serialize projection writes and capture using the existing pack-first
        // lock order. Caller-owned transactions retain this lock until commit.
        sqlx::query("SELECT id FROM pack WHERE id = $1 FOR NO KEY UPDATE")
            .bind(pack)
            .execute(&mut *connection)
            .await?;
        let owner: Option<i64> =
            sqlx::query_scalar("SELECT pack FROM pack_release WHERE id = $1 FOR SHARE")
                .bind(release)
                .fetch_optional(&mut *connection)
                .await?;
        if owner != Some(pack) {
            return Err(Error::validation(
                "Type snapshot release must belong to its pack",
            ));
        }
        let definitions = Self::list_by_pack(&mut *connection, pack)
            .await?
            .into_iter()
            .map(|row| (row.r#ref.clone(), row.definition()))
            .collect::<BTreeMap<_, _>>();
        let frozen: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM schema_type_release_snapshot WHERE release = $1)",
        )
        .bind(release)
        .fetch_one(&mut *connection)
        .await?;
        if frozen {
            let previous = Self::list_revisions(&mut *connection, release).await?;
            if previous.len() != definitions.len()
                || previous.iter().any(|revision| {
                    definitions
                        .get(&revision.type_ref)
                        .and_then(|definition| serde_json::to_value(definition).ok())
                        .as_ref()
                        != Some(&revision.definition)
                })
            {
                return Err(Error::validation(
                    "An immutable release already has a different effective named type view",
                ));
            }
            Self::refresh_type_references(connection, pack).await?;
            Self::refresh_consumer_references(connection, pack).await?;
            return Ok(());
        }
        for (type_ref, definition) in definitions {
            let digest = crate::value_schema::digest(&definition.schema)
                .map_err(|error| Error::validation(format!("Invalid named type body: {error}")))?;
            let body = serde_json::to_value(definition)
                .map_err(|_| Error::validation("Invalid named type metadata"))?;
            sqlx::query("INSERT INTO schema_type_revision (release, type_ref, definition, content_digest) VALUES ($1, $2, $3, $4)")
                .bind(release).bind(type_ref).bind(body).bind(digest).execute(&mut *connection).await?;
        }
        sqlx::query("INSERT INTO schema_type_release_snapshot (release) VALUES ($1)")
            .bind(release)
            .execute(&mut *connection)
            .await?;
        Self::refresh_type_references(connection, pack).await?;
        Self::refresh_consumer_references(connection, pack).await?;
        Ok(())
    }

    pub async fn list_revisions<'e, E>(executor: E, release: i64) -> Result<Vec<SchemaTypeRevision>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        Ok(sqlx::query_as(&format!("SELECT {REVISION_COLUMNS} FROM schema_type_revision WHERE release = $1 ORDER BY type_ref"))
            .bind(release).fetch_all(executor).await?)
    }

    /// Select same-pack bodies from an explicit immutable release, not mutable
    /// projections. New references to logically retired types remain forbidden.
    pub async fn release_view(
        connection: &mut PgConnection,
        release: i64,
        declaring_pack: &str,
    ) -> Result<RegistryView> {
        let (pack_ref, digest): (String, String) =
            sqlx::query_as("SELECT pack_ref, digest FROM pack_release WHERE id = $1")
                .bind(release)
                .fetch_optional(&mut *connection)
                .await?
                .ok_or_else(|| Error::not_found("pack release", "id", release.to_string()))?;
        #[derive(sqlx::FromRow)]
        struct ViewRow {
            id: i64,
            type_ref: String,
            definition: serde_json::Value,
            content_digest: String,
            active: bool,
        }
        let revisions: Vec<ViewRow> = sqlx::query_as(
            "SELECT r.id, r.type_ref, r.definition, r.content_digest, \
             EXISTS(SELECT 1 FROM schema_type t WHERE t.ref = r.type_ref AND t.retired_at IS NULL) AS active \
             FROM schema_type_revision r WHERE r.release = $1 ORDER BY r.type_ref"
        ).bind(release).fetch_all(&mut *connection).await?;
        let mut view = RegistryView::new();
        for revision in revisions {
            let definition: SchemaTypeDefinition = serde_json::from_value(revision.definition)
                .map_err(|_| Error::invalid_state("Invalid immutable named type definition"))?;
            definition.validate(&pack_ref)?;
            if crate::value_schema::digest(&definition.schema)
                .map_err(|_| Error::invalid_state("Invalid named type digest"))?
                != revision.content_digest
            {
                return Err(Error::invalid_state(
                    "Named type revision digest does not match its body",
                ));
            }
            let entry = if !revision.active {
                RegistryEntry::Retired
            } else if !definition.reference_allowed(declaring_pack) {
                RegistryEntry::Inaccessible
            } else {
                RegistryEntry::Available(TypeRevision {
                    source: SourceIdentity::Released {
                        component_ref: revision.type_ref.clone(),
                        release_id: release,
                        release_digest: digest.clone(),
                    },
                    revision_id: revision.id,
                    schema: definition.schema,
                })
            };
            view.insert(revision.type_ref, entry);
        }
        Ok(view)
    }
}

impl SchemaType {
    pub fn definition(&self) -> SchemaTypeDefinition {
        SchemaTypeDefinition {
            r#ref: self.r#ref.clone(),
            label: self.label.clone(),
            description: self.description.clone(),
            schema: self.schema.clone(),
            reference_visibility: self.reference_visibility,
            reference_allowed_pack_refs: self.reference_allowed_pack_refs.clone(),
        }
    }
}
