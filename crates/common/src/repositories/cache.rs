//! Persistence operations for owner-scoped external data caches.
//!
//! Cache records are never updated in place. Writers build a staging generation,
//! seal it, then atomically promote it through the namespace pointer.

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{Executor, PgPool, Postgres, QueryBuilder};
use std::collections::HashSet;

use crate::{
    config::CacheAdmissionConfig,
    models::{
        cache::{
            CacheEntry, CacheGeneration, CacheIngestChunk, CacheNamespace,
            CACHE_ENTRY_SELECT_COLUMNS, CACHE_GENERATION_SELECT_COLUMNS,
            CACHE_INGEST_CHUNK_SELECT_COLUMNS, CACHE_NAMESPACE_SELECT_COLUMNS,
        },
        CacheGenerationState, Id, OwnerType,
    },
    rbac::OwnerConstraint,
    Error, Result,
};

use super::{Create, FindById, List, Repository};

/// Largest accepted record count in one request. Records are still inserted in
/// smaller SQL batches so request and transaction memory remain bounded.
pub const MAX_INGEST_CHUNK_RECORDS: usize = 10_000;
/// Largest approximate encoded request payload accepted for one ingest chunk.
pub const MAX_INGEST_CHUNK_BYTES: usize = 32 * 1024 * 1024;
/// Maximum rows written by an individual INSERT statement.
pub const INGEST_INSERT_BATCH_SIZE: usize = 1_000;
/// Maximum items accepted for multi-ID reads.
pub const MAX_MULTI_LOOKUP_IDS: usize = 1_000;
/// Maximum logical JSON/text bytes returned by one multi-ID lookup.
pub const MAX_MULTI_LOOKUP_BYTES: i64 = 4 * 1024 * 1024;
/// Maximum rows returned by a single keyset scan call.
pub const MAX_SCAN_PAGE_SIZE: i64 = 1_000;
/// Repository-side ceiling for rows materialized by a scan. The API applies a
/// smaller serialized response limit and can continue from the returned key.
pub const MAX_SCAN_MATERIALIZATION_BYTES: i64 = 8 * 1024 * 1024;
/// Maximum generations considered by one cleanup selection.
pub const MAX_CLEANUP_SELECTION: i64 = 1_000;
pub const MAX_CACHE_ENTRY_VALUE_BYTES: usize = 1024 * 1024;
pub const MAX_CACHE_TEXT_BYTES: usize = 1024;
pub const MAX_CACHE_REASON_BYTES: usize = 4096;
/// Serializes aggregate cache admissions across API instances. The lock is
/// transaction-scoped and released automatically on commit or rollback.
const CACHE_ADMISSION_ADVISORY_LOCK_KEY: i64 = 7_821_101;

/// Canonical owner selector. API callers resolve owner references to these IDs
/// before calling cache repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheOwnerScope {
    pub owner_type: OwnerType,
    pub owner_identity: Option<Id>,
    pub owner_pack: Option<Id>,
    pub owner_pack_ref: Option<String>,
    pub owner_action: Option<Id>,
    pub owner_action_ref: Option<String>,
    pub owner_sensor: Option<Id>,
    pub owner_sensor_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CacheNamespacePage {
    pub items: Vec<CacheNamespace>,
    pub next_after_id: Option<Id>,
}

/// SQL-translatable cache read authority for cross-owner namespace listings.
#[derive(Debug, Clone)]
pub struct CacheNamespaceReadVisibility {
    pub identity_id: Id,
    pub grants: Vec<CacheNamespaceGrantFilter>,
}

/// The cache-row fields constrained by one readable RBAC grant.
#[derive(Debug, Clone, Default)]
pub struct CacheNamespaceGrantFilter {
    pub owner: Option<OwnerConstraint>,
    pub owner_types: Option<Vec<OwnerType>>,
    pub owner_refs: Option<Vec<String>>,
    pub namespace_refs: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct CacheGenerationPage {
    pub items: Vec<CacheGeneration>,
    pub next_before: Option<(DateTime<Utc>, Id)>,
}

#[derive(Debug, Clone)]
pub struct CacheEntryPage {
    pub generation: CacheGeneration,
    pub entries: Vec<CacheEntry>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheNamespaceFreshnessFilter {
    Fresh,
    Stale,
    Unpopulated,
}

impl CacheNamespaceFreshnessFilter {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unpopulated => "unpopulated",
        }
    }
}

impl CacheOwnerScope {
    pub fn system() -> Self {
        Self {
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
        }
    }

    pub fn identity(id: Id) -> Self {
        Self {
            owner_type: OwnerType::Identity,
            owner_identity: Some(id),
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
        }
    }

    pub fn pack(id: Id, reference: Option<String>) -> Self {
        Self {
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(id),
            owner_pack_ref: reference,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
        }
    }

    pub fn action(id: Id, reference: Option<String>) -> Self {
        Self {
            owner_type: OwnerType::Action,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: Some(id),
            owner_action_ref: reference,
            owner_sensor: None,
            owner_sensor_ref: None,
        }
    }

    pub fn sensor(id: Id, reference: Option<String>) -> Self {
        Self {
            owner_type: OwnerType::Sensor,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: Some(id),
            owner_sensor_ref: reference,
        }
    }

    /// Validates the selector and returns the database's canonical owner key.
    pub fn canonical_owner(&self) -> Result<String> {
        validate_optional_text(
            self.owner_pack_ref.as_deref(),
            MAX_CACHE_TEXT_BYTES,
            "cache owner pack ref",
        )?;
        validate_optional_text(
            self.owner_action_ref.as_deref(),
            MAX_CACHE_TEXT_BYTES,
            "cache owner action ref",
        )?;
        validate_optional_text(
            self.owner_sensor_ref.as_deref(),
            MAX_CACHE_TEXT_BYTES,
            "cache owner sensor ref",
        )?;
        let id_count = [
            self.owner_identity,
            self.owner_pack,
            self.owner_action,
            self.owner_sensor,
        ]
        .into_iter()
        .flatten()
        .count();

        let invalid_refs = match self.owner_type {
            OwnerType::System | OwnerType::Identity => {
                self.owner_pack_ref.is_some()
                    || self.owner_action_ref.is_some()
                    || self.owner_sensor_ref.is_some()
            }
            OwnerType::Pack => self.owner_action_ref.is_some() || self.owner_sensor_ref.is_some(),
            OwnerType::Action => self.owner_pack_ref.is_some() || self.owner_sensor_ref.is_some(),
            OwnerType::Sensor => self.owner_pack_ref.is_some() || self.owner_action_ref.is_some(),
        };

        if invalid_refs {
            return Err(Error::validation(
                "cache owner references do not match the selected owner type",
            ));
        }

        match self.owner_type {
            OwnerType::System if id_count == 0 => Ok("system".to_string()),
            OwnerType::Identity if id_count == 1 => self
                .owner_identity
                .map(|id| id.to_string())
                .ok_or_else(|| Error::validation("identity cache owner requires owner_identity")),
            OwnerType::Pack if id_count == 1 => self
                .owner_pack
                .map(|id| id.to_string())
                .ok_or_else(|| Error::validation("pack cache owner requires owner_pack")),
            OwnerType::Action if id_count == 1 => self
                .owner_action
                .map(|id| id.to_string())
                .ok_or_else(|| Error::validation("action cache owner requires owner_action")),
            OwnerType::Sensor if id_count == 1 => self
                .owner_sensor
                .map(|id| id.to_string())
                .ok_or_else(|| Error::validation("sensor cache owner requires owner_sensor")),
            OwnerType::System => Err(Error::validation(
                "system cache owner cannot include a canonical owner ID",
            )),
            _ => Err(Error::validation(
                "cache owner must include exactly one canonical ID matching owner_type",
            )),
        }
    }
}

/// Namespace lifecycle and storage limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheNamespacePolicy {
    pub freshness_target_seconds: i64,
    pub max_records_per_generation: i64,
    pub max_generation_bytes: i64,
    pub max_retained_bytes: i64,
    pub max_retained_generations: i32,
    pub max_staging_generations: i32,
}

impl Default for CacheNamespacePolicy {
    fn default() -> Self {
        Self {
            freshness_target_seconds: 3600,
            max_records_per_generation: 200_000,
            max_generation_bytes: 512 * 1024 * 1024,
            max_retained_bytes: 2 * 1024 * 1024 * 1024,
            max_retained_generations: 5,
            max_staging_generations: 2,
        }
    }
}

impl CacheNamespacePolicy {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.freshness_target_seconds < 0
            || self.max_records_per_generation < 0
            || self.max_generation_bytes < 0
            || self.max_retained_bytes < 0
            || self.max_retained_generations < 2
            || self.max_staging_generations < 1
        {
            return Err(Error::validation(
                "cache namespace limits must be nonnegative, max_retained_generations must be at least 2, and max_staging_generations must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CreateCacheNamespaceInput {
    pub owner: CacheOwnerScope,
    pub namespace: String,
    pub policy: CacheNamespacePolicy,
}

/// Desired state for one namespace declared by a pack cache definition.
#[derive(Debug, Clone)]
pub struct ManagedCacheNamespaceDefinition {
    pub definition_ref: String,
    pub owner: CacheOwnerScope,
    pub namespace: String,
    pub policy: CacheNamespacePolicy,
}

/// Aggregate result of applying pack-managed namespace definitions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ManagedCacheNamespaceUpsertSummary {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// Atomic result of removing pack-owned action and sensor components together
/// with every live cache namespace they own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemovedCacheOwnerCleanupSummary {
    pub tombstoned_namespaces: u64,
    pub deleted_actions: u64,
    pub deleted_sensors: u64,
}

#[derive(Debug, Clone)]
pub struct CreateCacheGenerationInput {
    pub namespace: Id,
    pub client_refresh_id: String,
    pub expected_active_generation: Option<Id>,
    pub expected_chunk_count: i32,
    pub expected_count: Option<i64>,
    pub expected_bytes: Option<i64>,
    /// Whole-generation checksum verification is intentionally not supported
    /// until canonical JSON encoding is defined.
    pub checksum_algorithm: Option<String>,
    pub checksum: Option<String>,
    pub source_revision: Option<String>,
    pub created_by: Option<Id>,
}

#[derive(Debug, Clone)]
pub enum CreateCacheGenerationResult {
    Created(CacheGeneration),
    Existing(CacheGeneration),
}

#[derive(Debug, Clone)]
pub struct CacheEntryInput {
    pub external_id: String,
    pub value: JsonValue,
    pub source_updated_at: Option<DateTime<Utc>>,
    pub source_checksum: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct SealCacheGenerationInput {
    pub expected_chunk_count: i32,
    pub expected_count: Option<i64>,
    pub expected_bytes: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum InsertCacheChunkResult {
    Inserted(CacheIngestChunk),
    Replayed(CacheIngestChunk),
}

#[derive(Debug, Clone)]
pub struct CachePromotionResult {
    pub namespace: CacheNamespace,
    pub activated_generation: CacheGeneration,
    pub retired_generation: Option<Id>,
    pub replayed: bool,
}

pub struct CacheNamespaceRepository;

impl Repository for CacheNamespaceRepository {
    type Entity = CacheNamespace;

    fn table_name() -> &'static str {
        "cache_namespace"
    }
}

#[async_trait::async_trait]
impl FindById for CacheNamespaceRepository {
    async fn find_by_id<'e, E>(executor: E, id: Id) -> Result<Option<CacheNamespace>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query =
            format!("SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace WHERE id = $1");
        sqlx::query_as::<_, CacheNamespace>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for CacheNamespaceRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<CacheNamespace>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
             WHERE tombstoned_at IS NULL ORDER BY owner_type, owner, namespace LIMIT 1000"
        );
        sqlx::query_as::<_, CacheNamespace>(&query)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for CacheNamespaceRepository {
    type CreateInput = CreateCacheNamespaceInput;

    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<CacheNamespace>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        validate_namespace_name(&input.namespace)?;
        input.policy.validate()?;
        input.owner.canonical_owner()?;
        insert_namespace(executor, &input, None, None, None).await
    }
}

impl CacheNamespaceRepository {
    /// Creates an API-managed namespace while preserving the historical
    /// conflict behavior for owner/name slots still occupied by a tombstoned
    /// namespace. Pack deployment uses the provenance-aware upsert path below.
    pub async fn create_api(
        pool: &PgPool,
        input: CreateCacheNamespaceInput,
    ) -> Result<CacheNamespace> {
        Self::create_api_with_policy(pool, input, &CacheAdmissionConfig::default()).await
    }

    pub async fn create_api_with_policy(
        pool: &PgPool,
        input: CreateCacheNamespaceInput,
        admission: &CacheAdmissionConfig,
    ) -> Result<CacheNamespace> {
        validate_namespace_name(&input.namespace)?;
        input.policy.validate()?;
        let canonical_owner = input.owner.canonical_owner()?;

        let mut tx = pool.begin().await?;
        lock_cache_admission(&mut tx).await?;
        let existing_id = sqlx::query_scalar::<_, Id>(
            "SELECT id FROM cache_namespace \
             WHERE owner_type = $1 AND owner = $2 AND namespace = $3 \
             ORDER BY tombstoned_at NULLS FIRST, id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(input.owner.owner_type)
        .bind(&canonical_owner)
        .bind(&input.namespace)
        .fetch_optional(&mut *tx)
        .await?;
        if existing_id.is_some() {
            tx.rollback().await?;
            return Err(Error::already_exists(
                "cache_namespace",
                "owner+namespace",
                input.namespace,
            ));
        }

        ensure_namespace_admission(&mut tx, input.owner.owner_type, &canonical_owner, admission)
            .await?;

        let created = insert_namespace(&mut *tx, &input, None, None, None).await?;
        tx.commit().await?;
        Ok(created)
    }

    /// Applies pack-managed definitions atomically. Existing live definitions
    /// retain their namespace ID and generations; only policy fields may
    /// change. A tombstoned predecessor is never resurrected.
    pub async fn upsert_managed_definitions(
        pool: &PgPool,
        managing_pack: Id,
        managing_pack_ref: &str,
        definitions: &[ManagedCacheNamespaceDefinition],
        admission: &CacheAdmissionConfig,
    ) -> Result<ManagedCacheNamespaceUpsertSummary> {
        let mut tx = pool.begin().await?;
        let summary = Self::upsert_managed_definitions_in_transaction(
            &mut tx,
            managing_pack,
            managing_pack_ref,
            definitions,
            admission,
        )
        .await?;
        tx.commit().await?;
        Ok(summary)
    }

    pub async fn upsert_managed_definitions_in_transaction(
        connection: &mut sqlx::PgConnection,
        managing_pack: Id,
        managing_pack_ref: &str,
        definitions: &[ManagedCacheNamespaceDefinition],
        admission: &CacheAdmissionConfig,
    ) -> Result<ManagedCacheNamespaceUpsertSummary> {
        if managing_pack_ref.trim().is_empty() {
            return Err(Error::validation(
                "managed cache namespaces require a managing pack ref",
            ));
        }
        validate_required_text(
            managing_pack_ref,
            MAX_CACHE_TEXT_BYTES,
            "managed cache namespace pack ref",
        )?;

        for definition in definitions {
            validate_managed_definition(definition)?;
        }

        lock_cache_admission(connection).await?;
        let mut summary = ManagedCacheNamespaceUpsertSummary::default();

        for definition in definitions {
            let query = format!(
                "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
                 WHERE managing_pack_ref = $1 AND definition_ref = $2 \
                   AND tombstoned_at IS NULL FOR UPDATE"
            );
            let existing = sqlx::query_as::<_, CacheNamespace>(&query)
                .bind(managing_pack_ref)
                .bind(&definition.definition_ref)
                .fetch_optional(&mut *connection)
                .await?;

            if let Some(existing) = existing {
                let canonical_owner = definition.owner.canonical_owner()?;
                if existing.managing_pack != Some(managing_pack)
                    || existing.owner_type != definition.owner.owner_type
                    || existing.owner != canonical_owner
                    || existing.namespace != definition.namespace
                {
                    return Err(Error::validation(format!(
                        "cache definition '{}' cannot change its namespace or owner",
                        definition.definition_ref
                    )));
                }

                if namespace_policy(&existing) == definition.policy {
                    summary.unchanged += 1;
                    continue;
                }

                let update = format!(
                    "UPDATE cache_namespace SET freshness_target_seconds = $2, \
                     max_records_per_generation = $3, max_generation_bytes = $4, \
                     max_retained_bytes = $5, max_retained_generations = $6, \
                     max_staging_generations = $7 \
                     WHERE id = $1 AND tombstoned_at IS NULL \
                     RETURNING {CACHE_NAMESPACE_SELECT_COLUMNS}"
                );
                sqlx::query_as::<_, CacheNamespace>(&update)
                    .bind(existing.id)
                    .bind(definition.policy.freshness_target_seconds)
                    .bind(definition.policy.max_records_per_generation)
                    .bind(definition.policy.max_generation_bytes)
                    .bind(definition.policy.max_retained_bytes)
                    .bind(definition.policy.max_retained_generations)
                    .bind(definition.policy.max_staging_generations)
                    .fetch_one(&mut *connection)
                    .await?;
                summary.updated += 1;
                continue;
            }

            let input = CreateCacheNamespaceInput {
                owner: definition.owner.clone(),
                namespace: definition.namespace.clone(),
                policy: definition.policy.clone(),
            };
            let canonical_owner = input.owner.canonical_owner()?;
            ensure_namespace_admission(
                &mut *connection,
                input.owner.owner_type,
                &canonical_owner,
                admission,
            )
            .await?;
            insert_namespace(
                &mut *connection,
                &input,
                Some(&definition.definition_ref),
                Some(managing_pack),
                Some(managing_pack_ref),
            )
            .await?;
            summary.created += 1;
        }

        Ok(summary)
    }

    pub async fn resolve<'e, E>(
        executor: E,
        owner: &CacheOwnerScope,
        namespace: &str,
    ) -> Result<Option<CacheNamespace>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        validate_namespace_name(namespace)?;
        let canonical_owner = owner.canonical_owner()?;
        let query = format!(
            "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
             WHERE owner_type = $1 AND owner = $2 AND namespace = $3 AND tombstoned_at IS NULL"
        );
        sqlx::query_as::<_, CacheNamespace>(&query)
            .bind(owner.owner_type)
            .bind(canonical_owner)
            .bind(namespace)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn resolve_managed_definition<'e, E>(
        executor: E,
        managing_pack_ref: &str,
        definition_ref: &str,
    ) -> Result<Option<CacheNamespace>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query = format!(
            "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
             WHERE managing_pack_ref = $1 AND definition_ref = $2 \
               AND tombstoned_at IS NULL"
        );
        sqlx::query_as::<_, CacheNamespace>(&query)
            .bind(managing_pack_ref)
            .bind(definition_ref)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Resolves a namespace regardless of tombstone state. Callers must have
    /// already authorized the canonical owner scope and must handle
    /// `tombstoned_at` explicitly; normal read paths should use [`Self::resolve`].
    pub async fn resolve_including_tombstoned<'e, E>(
        executor: E,
        owner: &CacheOwnerScope,
        namespace: &str,
    ) -> Result<Option<CacheNamespace>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        validate_namespace_name(namespace)?;
        let canonical_owner = owner.canonical_owner()?;
        let query = format!(
            "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
             WHERE owner_type = $1 AND owner = $2 AND namespace = $3 \
             ORDER BY tombstoned_at NULLS FIRST, id DESC LIMIT 1"
        );
        sqlx::query_as::<_, CacheNamespace>(&query)
            .bind(owner.owner_type)
            .bind(canonical_owner)
            .bind(namespace)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    pub async fn list_metadata<'e, E>(
        executor: E,
        owner: Option<&CacheOwnerScope>,
        limit: i64,
    ) -> Result<Vec<CacheNamespace>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        Ok(Self::list_metadata_page(executor, owner, None, limit)
            .await?
            .items)
    }

    /// Stable ID-keyset page for API metadata pagination and complete
    /// supervisor traversal.
    pub async fn list_metadata_page<'e, E>(
        executor: E,
        owner: Option<&CacheOwnerScope>,
        after_id: Option<Id>,
        limit: i64,
    ) -> Result<CacheNamespacePage>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let limit = bounded_limit(limit, MAX_CLEANUP_SELECTION, "namespace list")?;
        let fetch_limit = limit + 1;
        if let Some(owner) = owner {
            let canonical_owner = owner.canonical_owner()?;
            let query = format!(
                "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
                 WHERE owner_type = $1 AND owner = $2 AND tombstoned_at IS NULL \
                   AND ($3::BIGINT IS NULL OR id > $3) \
                 ORDER BY id LIMIT $4"
            );
            let items = sqlx::query_as::<_, CacheNamespace>(&query)
                .bind(owner.owner_type)
                .bind(canonical_owner)
                .bind(after_id)
                .bind(fetch_limit)
                .fetch_all(executor)
                .await?;
            Ok(namespace_page(items, limit))
        } else {
            let query = format!(
                "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
                 WHERE tombstoned_at IS NULL AND ($1::BIGINT IS NULL OR id > $1) \
                 ORDER BY id LIMIT $2"
            );
            let items = sqlx::query_as::<_, CacheNamespace>(&query)
                .bind(after_id)
                .bind(fetch_limit)
                .fetch_all(executor)
                .await?;
            Ok(namespace_page(items, limit))
        }
    }

    /// Lists one canonical owner scope while applying an authorization-derived
    /// namespace allow-list in SQL. `None` means every namespace in the owner
    /// scope is visible; `Some` is an exact namespace list and an empty list
    /// fails closed without querying rows.
    pub async fn list_metadata_visible<'e, E>(
        executor: E,
        owner: &CacheOwnerScope,
        visible_namespaces: Option<&[String]>,
        limit: i64,
    ) -> Result<Vec<CacheNamespace>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        Ok(
            Self::list_metadata_visible_page(executor, owner, visible_namespaces, None, limit)
                .await?
                .items,
        )
    }

    /// Authorization-filtered namespace metadata using the same stable ID
    /// keyset as [`Self::list_metadata_page`].
    pub async fn list_metadata_visible_page<'e, E>(
        executor: E,
        owner: &CacheOwnerScope,
        visible_namespaces: Option<&[String]>,
        after_id: Option<Id>,
        limit: i64,
    ) -> Result<CacheNamespacePage>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let limit = bounded_limit(limit, MAX_CLEANUP_SELECTION, "namespace list")?;
        let fetch_limit = limit + 1;
        let canonical_owner = owner.canonical_owner()?;
        match visible_namespaces {
            Some([]) => Ok(CacheNamespacePage {
                items: Vec::new(),
                next_after_id: None,
            }),
            Some(namespaces) => {
                let query = format!(
                    "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
                     WHERE owner_type = $1 AND owner = $2 AND tombstoned_at IS NULL \
                       AND namespace = ANY($3) AND ($4::BIGINT IS NULL OR id > $4) \
                     ORDER BY id LIMIT $5"
                );
                let items = sqlx::query_as::<_, CacheNamespace>(&query)
                    .bind(owner.owner_type)
                    .bind(canonical_owner)
                    .bind(namespaces)
                    .bind(after_id)
                    .bind(fetch_limit)
                    .fetch_all(executor)
                    .await?;
                Ok(namespace_page(items, limit))
            }
            None => {
                let query = format!(
                    "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace \
                     WHERE owner_type = $1 AND owner = $2 AND tombstoned_at IS NULL \
                       AND ($3::BIGINT IS NULL OR id > $3) \
                     ORDER BY id LIMIT $4"
                );
                let items = sqlx::query_as::<_, CacheNamespace>(&query)
                    .bind(owner.owner_type)
                    .bind(canonical_owner)
                    .bind(after_id)
                    .bind(fetch_limit)
                    .fetch_all(executor)
                    .await?;
                Ok(namespace_page(items, limit))
            }
        }
    }

    /// Authorization-filtered namespace metadata with server-side namespace
    /// and freshness filters. The stable ID keyset is applied after all
    /// filters so cursors never skip matching rows.
    #[allow(clippy::too_many_arguments)]
    pub async fn list_metadata_visible_filtered_page<'e, E>(
        executor: E,
        owner: &CacheOwnerScope,
        visible_namespaces: Option<&[String]>,
        after_id: Option<Id>,
        namespace_contains: Option<&str>,
        freshness: Option<CacheNamespaceFreshnessFilter>,
        limit: i64,
    ) -> Result<CacheNamespacePage>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let limit = bounded_limit(limit, MAX_CLEANUP_SELECTION, "namespace list")?;
        let fetch_limit = limit + 1;
        let canonical_owner = owner.canonical_owner()?;
        let namespace_pattern = namespace_contains.map(contains_like_pattern);
        let freshness = freshness.map(CacheNamespaceFreshnessFilter::as_str);
        let freshness_predicate = "\
            ($6::TEXT IS NULL \
             OR ($6 = 'unpopulated' AND n.active_generation IS NULL) \
             OR ($6 = 'fresh' AND n.active_generation IS NOT NULL \
                 AND active.id IS NOT NULL \
                 AND (n.freshness_target_seconds <= 0 OR active.activated IS NULL \
                      OR active.activated >= NOW() \
                         - n.freshness_target_seconds * INTERVAL '1 second')) \
             OR ($6 = 'stale' AND n.active_generation IS NOT NULL \
                 AND active.id IS NOT NULL AND n.freshness_target_seconds > 0 \
                 AND active.activated IS NOT NULL \
                 AND active.activated < NOW() \
                     - n.freshness_target_seconds * INTERVAL '1 second'))";

        match visible_namespaces {
            Some([]) => Ok(CacheNamespacePage {
                items: Vec::new(),
                next_after_id: None,
            }),
            Some(namespaces) => {
                let query = format!(
                    "SELECT {} FROM cache_namespace n \
                     LEFT JOIN cache_generation active ON active.id = n.active_generation \
                     WHERE n.owner_type = $1 AND n.owner = $2 AND n.tombstoned_at IS NULL \
                       AND n.namespace = ANY($3) AND ($4::BIGINT IS NULL OR n.id > $4) \
                       AND ($5::TEXT IS NULL OR n.namespace LIKE $5 ESCAPE '\\') \
                       AND {freshness_predicate} \
                     ORDER BY n.id LIMIT $7",
                    qualified_columns("n", CACHE_NAMESPACE_SELECT_COLUMNS),
                );
                let items = sqlx::query_as::<_, CacheNamespace>(&query)
                    .bind(owner.owner_type)
                    .bind(canonical_owner)
                    .bind(namespaces)
                    .bind(after_id)
                    .bind(&namespace_pattern)
                    .bind(freshness)
                    .bind(fetch_limit)
                    .fetch_all(executor)
                    .await?;
                Ok(namespace_page(items, limit))
            }
            None => {
                let freshness_predicate = freshness_predicate.replace("$6", "$5");
                let query = format!(
                    "SELECT {} FROM cache_namespace n \
                     LEFT JOIN cache_generation active ON active.id = n.active_generation \
                     WHERE n.owner_type = $1 AND n.owner = $2 AND n.tombstoned_at IS NULL \
                       AND ($3::BIGINT IS NULL OR n.id > $3) \
                       AND ($4::TEXT IS NULL OR n.namespace LIKE $4 ESCAPE '\\') \
                       AND {freshness_predicate} \
                     ORDER BY n.id LIMIT $6",
                    qualified_columns("n", CACHE_NAMESPACE_SELECT_COLUMNS),
                );
                let items = sqlx::query_as::<_, CacheNamespace>(&query)
                    .bind(owner.owner_type)
                    .bind(canonical_owner)
                    .bind(after_id)
                    .bind(&namespace_pattern)
                    .bind(freshness)
                    .bind(fetch_limit)
                    .fetch_all(executor)
                    .await?;
                Ok(namespace_page(items, limit))
            }
        }
    }

    /// Lists namespaces across all owner scopes while applying the caller's
    /// compiled RBAC visibility before keyset pagination.
    pub async fn list_metadata_all_owners_visible_filtered_page<'e, E>(
        executor: E,
        visibility: &CacheNamespaceReadVisibility,
        after_id: Option<Id>,
        namespace_contains: Option<&str>,
        freshness: Option<CacheNamespaceFreshnessFilter>,
        limit: i64,
    ) -> Result<CacheNamespacePage>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let limit = bounded_limit(limit, MAX_CLEANUP_SELECTION, "namespace list")?;
        if visibility.grants.is_empty() {
            return Ok(CacheNamespacePage {
                items: Vec::new(),
                next_after_id: None,
            });
        }

        let mut query: QueryBuilder<'_, Postgres> = QueryBuilder::new(format!(
            "SELECT {} FROM cache_namespace n \
             LEFT JOIN cache_generation active ON active.id = n.active_generation \
             WHERE n.tombstoned_at IS NULL AND ",
            qualified_columns("n", CACHE_NAMESPACE_SELECT_COLUMNS),
        ));

        query.push("(n.owner_type <> ");
        query.push_bind(OwnerType::Identity);
        query.push(" OR n.owner_identity = ");
        query.push_bind(visibility.identity_id);
        query.push(") AND ");
        push_cache_namespace_visibility_clause(&mut query, visibility);

        if let Some(after_id) = after_id {
            query.push(" AND n.id > ");
            query.push_bind(after_id);
        }
        if let Some(namespace_contains) = namespace_contains {
            query.push(" AND n.namespace LIKE ");
            query.push_bind(contains_like_pattern(namespace_contains));
            query.push(" ESCAPE '\\'");
        }
        push_cache_namespace_freshness_clause(&mut query, freshness);

        query.push(" ORDER BY n.id LIMIT ");
        query.push_bind(limit + 1);
        let items = query
            .build_query_as::<CacheNamespace>()
            .fetch_all(executor)
            .await?;
        Ok(namespace_page(items, limit))
    }

    pub async fn update_policy(
        pool: &PgPool,
        namespace_id: Id,
        policy: &CacheNamespacePolicy,
    ) -> Result<CacheNamespace> {
        policy.validate()?;
        let query = format!(
            "UPDATE cache_namespace SET freshness_target_seconds = $2, \
             max_records_per_generation = $3, max_generation_bytes = $4, \
             max_retained_bytes = $5, max_retained_generations = $6, \
             max_staging_generations = $7 \
             WHERE id = $1 AND tombstoned_at IS NULL \
             RETURNING {CACHE_NAMESPACE_SELECT_COLUMNS}"
        );
        sqlx::query_as::<_, CacheNamespace>(&query)
            .bind(namespace_id)
            .bind(policy.freshness_target_seconds)
            .bind(policy.max_records_per_generation)
            .bind(policy.max_generation_bytes)
            .bind(policy.max_retained_bytes)
            .bind(policy.max_retained_generations)
            .bind(policy.max_staging_generations)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| Error::not_found("cache_namespace", "id", namespace_id.to_string()))
    }

    /// Makes a namespace unreadable immediately and moves unpublished
    /// generations to failed. Its data is reclaimed by bounded cleanup calls.
    pub async fn tombstone(pool: &PgPool, namespace_id: Id) -> Result<bool> {
        Self::tombstone_with_reason(pool, namespace_id, "namespace deleted").await
    }

    pub async fn tombstone_with_reason(
        pool: &PgPool,
        namespace_id: Id,
        reason: &str,
    ) -> Result<bool> {
        validate_tombstone_reason(reason)?;
        let mut tx = pool.begin().await?;
        let tombstoned = tombstone_namespace_in_transaction(&mut tx, namespace_id, reason).await?;
        tx.commit().await?;
        Ok(tombstoned)
    }

    /// Tombstones live definitions removed from one pack's `caches/*.yaml`
    /// set. API-created namespaces have no definition provenance and are never
    /// selected by this operation.
    pub async fn tombstone_managed_by_pack_excluding(
        pool: &PgPool,
        managing_pack: Id,
        keep_definition_refs: &[String],
    ) -> Result<u64> {
        let mut tx = pool.begin().await?;
        let count = Self::tombstone_managed_by_pack_excluding_in_transaction(
            &mut tx,
            managing_pack,
            keep_definition_refs,
        )
        .await?;
        tx.commit().await?;
        Ok(count)
    }

    pub async fn tombstone_managed_by_pack_excluding_in_transaction(
        connection: &mut sqlx::PgConnection,
        managing_pack: Id,
        keep_definition_refs: &[String],
    ) -> Result<u64> {
        let ids = if keep_definition_refs.is_empty() {
            sqlx::query_scalar::<_, Id>(
                "SELECT id FROM cache_namespace \
                 WHERE managing_pack = $1 AND definition_ref IS NOT NULL \
                   AND tombstoned_at IS NULL ORDER BY id",
            )
            .bind(managing_pack)
            .fetch_all(&mut *connection)
            .await?
        } else {
            sqlx::query_scalar::<_, Id>(
                "SELECT id FROM cache_namespace \
                 WHERE managing_pack = $1 AND definition_ref IS NOT NULL \
                   AND definition_ref != ALL($2) AND tombstoned_at IS NULL ORDER BY id",
            )
            .bind(managing_pack)
            .bind(keep_definition_refs)
            .fetch_all(&mut *connection)
            .await?
        };

        let mut count = 0;
        for id in ids {
            if tombstone_namespace_in_transaction(connection, id, "pack cache definition removed")
                .await?
            {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Removes stale pack action/sensor components and tombstones every live
    /// namespace they own in one transaction.
    ///
    /// Owner rows are locked before namespace selection. That prevents a
    /// concurrent API request from attaching a new live namespace after the
    /// tombstone set has been determined but before the owners are deleted.
    /// API-created and pack-managed namespaces are both included.
    pub async fn delete_removed_pack_owners(
        pool: &PgPool,
        pack_id: Id,
        keep_action_refs: &[String],
        keep_sensor_refs: &[String],
    ) -> Result<RemovedCacheOwnerCleanupSummary> {
        let mut tx = pool.begin().await?;
        let summary = Self::delete_removed_pack_owners_in_transaction(
            &mut tx,
            pack_id,
            keep_action_refs,
            keep_sensor_refs,
        )
        .await?;
        tx.commit().await?;
        Ok(summary)
    }

    pub async fn delete_removed_pack_owners_in_transaction(
        connection: &mut sqlx::PgConnection,
        pack_id: Id,
        keep_action_refs: &[String],
        keep_sensor_refs: &[String],
    ) -> Result<RemovedCacheOwnerCleanupSummary> {
        let action_ids = sqlx::query_scalar::<_, Id>(
            "SELECT id FROM action \
             WHERE pack = $1 AND is_adhoc = false \
               AND (cardinality($2::TEXT[]) = 0 OR ref != ALL($2)) \
             ORDER BY id FOR UPDATE",
        )
        .bind(pack_id)
        .bind(keep_action_refs)
        .fetch_all(&mut *connection)
        .await?;
        let sensor_ids = sqlx::query_scalar::<_, Id>(
            "SELECT id FROM sensor \
             WHERE pack = $1 \
               AND (cardinality($2::TEXT[]) = 0 OR ref != ALL($2)) \
             ORDER BY id FOR UPDATE",
        )
        .bind(pack_id)
        .bind(keep_sensor_refs)
        .fetch_all(&mut *connection)
        .await?;

        let namespace_ids = sqlx::query_scalar::<_, Id>(
            "SELECT id FROM cache_namespace \
             WHERE tombstoned_at IS NULL \
               AND (owner_action = ANY($1::BIGINT[]) OR owner_sensor = ANY($2::BIGINT[])) \
             ORDER BY id",
        )
        .bind(&action_ids)
        .bind(&sensor_ids)
        .fetch_all(&mut *connection)
        .await?;

        let mut tombstoned_namespaces = 0;
        for id in namespace_ids {
            if tombstone_namespace_in_transaction(connection, id, "cache owner removed from pack")
                .await?
            {
                tombstoned_namespaces += 1;
            }
        }

        let deleted_sensors = sqlx::query("DELETE FROM sensor WHERE id = ANY($1::BIGINT[])")
            .bind(&sensor_ids)
            .execute(&mut *connection)
            .await?
            .rows_affected();
        let deleted_actions = sqlx::query("DELETE FROM action WHERE id = ANY($1::BIGINT[])")
            .bind(&action_ids)
            .execute(&mut *connection)
            .await?
            .rows_affected();

        Ok(RemovedCacheOwnerCleanupSummary {
            tombstoned_namespaces,
            deleted_actions,
            deleted_sensors,
        })
    }

    /// Tombstones every namespace affected by deleting a pack. The caller
    /// keeps this transaction open through the pack delete so owner FKs can be
    /// cleared only after all namespaces are unreadable.
    pub async fn tombstone_for_pack_deletion(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        pack_id: Id,
    ) -> Result<u64> {
        let ids = sqlx::query_scalar::<_, Id>(
            "SELECT n.id FROM cache_namespace n \
             WHERE n.tombstoned_at IS NULL AND ( \
                 n.managing_pack = $1 OR n.owner_pack = $1 \
                 OR n.owner_action IN (SELECT a.id FROM action a WHERE a.pack = $1) \
                 OR n.owner_sensor IN (SELECT s.id FROM sensor s WHERE s.pack = $1) \
             ) ORDER BY n.id",
        )
        .bind(pack_id)
        .fetch_all(&mut **tx)
        .await?;

        let mut count = 0;
        for id in ids {
            if tombstone_namespace_in_transaction(tx, id, "owning pack deleted").await? {
                count += 1;
            }
        }
        Ok(count)
    }

    pub async fn tombstone_for_action_deletion(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        action_id: Id,
    ) -> Result<u64> {
        let ids = sqlx::query_scalar::<_, Id>(
            "SELECT id FROM cache_namespace \
             WHERE owner_action = $1 AND tombstoned_at IS NULL ORDER BY id",
        )
        .bind(action_id)
        .fetch_all(&mut **tx)
        .await?;

        let mut count = 0;
        for id in ids {
            if tombstone_namespace_in_transaction(tx, id, "owning action deleted").await? {
                count += 1;
            }
        }
        Ok(count)
    }

    pub async fn tombstone_for_sensor_deletion(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        sensor_id: Id,
    ) -> Result<u64> {
        let ids = sqlx::query_scalar::<_, Id>(
            "SELECT id FROM cache_namespace \
             WHERE owner_sensor = $1 AND tombstoned_at IS NULL ORDER BY id",
        )
        .bind(sensor_id)
        .fetch_all(&mut **tx)
        .await?;

        let mut count = 0;
        for id in ids {
            if tombstone_namespace_in_transaction(tx, id, "owning sensor deleted").await? {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Deletes only an already tombstoned namespace with no generations left.
    pub async fn delete_tombstoned_if_empty(pool: &PgPool, namespace_id: Id) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM cache_namespace n \
             WHERE n.id = $1 AND n.tombstoned_at IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM cache_generation g WHERE g.namespace = n.id)",
        )
        .bind(namespace_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Independently removes a bounded batch of tombstoned namespaces that
    /// already have no generations, including namespaces that were never populated.
    pub async fn delete_empty_tombstoned_batch(pool: &PgPool, limit: i64) -> Result<u64> {
        let limit = bounded_limit(limit, MAX_CLEANUP_SELECTION, "tombstoned namespace cleanup")?;
        let result = sqlx::query(
            "WITH candidates AS ( \
                 SELECT n.id FROM cache_namespace n \
                  WHERE n.tombstoned_at IS NOT NULL \
                    AND NOT EXISTS (SELECT 1 FROM cache_generation g WHERE g.namespace = n.id) \
                  ORDER BY n.tombstoned_at, n.id LIMIT $1 \
                  FOR UPDATE OF n SKIP LOCKED \
             ) \
             DELETE FROM cache_namespace n USING candidates c WHERE n.id = c.id",
        )
        .bind(limit)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}

pub struct CacheGenerationRepository;

impl Repository for CacheGenerationRepository {
    type Entity = CacheGeneration;

    fn table_name() -> &'static str {
        "cache_generation"
    }
}

#[async_trait::async_trait]
impl FindById for CacheGenerationRepository {
    async fn find_by_id<'e, E>(executor: E, id: Id) -> Result<Option<CacheGeneration>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let query =
            format!("SELECT {CACHE_GENERATION_SELECT_COLUMNS} FROM cache_generation WHERE id = $1");
        sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }
}

impl CacheGenerationRepository {
    /// Loads and share-locks a generation for transactional consumers that
    /// must establish a durable read pin before cleanup can begin.
    pub async fn find_by_id_for_share(
        conn: &mut sqlx::PgConnection,
        generation_id: Id,
    ) -> Result<Option<CacheGeneration>> {
        let query = format!(
            "SELECT {CACHE_GENERATION_SELECT_COLUMNS} FROM cache_generation \
             WHERE id = $1 FOR SHARE"
        );
        sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(generation_id)
            .fetch_optional(conn)
            .await
            .map_err(Into::into)
    }

    /// Batch-loads generation metadata for namespace list enrichment without
    /// one query per active generation.
    pub async fn find_by_ids(pool: &PgPool, ids: &[Id]) -> Result<Vec<CacheGeneration>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let query = format!(
            "SELECT {CACHE_GENERATION_SELECT_COLUMNS} FROM cache_generation WHERE id = ANY($1)"
        );
        sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(ids)
            .fetch_all(pool)
            .await
            .map_err(Into::into)
    }

    /// Creates a staging generation or returns the prior matching generation
    /// for an idempotent client refresh retry.
    pub async fn create_or_get(
        pool: &PgPool,
        input: &CreateCacheGenerationInput,
    ) -> Result<CreateCacheGenerationResult> {
        Self::create_or_get_with_policy(pool, input, &CacheAdmissionConfig::default()).await
    }

    pub async fn create_or_get_with_policy(
        pool: &PgPool,
        input: &CreateCacheGenerationInput,
        admission: &CacheAdmissionConfig,
    ) -> Result<CreateCacheGenerationResult> {
        validate_generation_input(input)?;
        let mut tx = pool.begin().await?;
        lock_cache_admission(&mut tx).await?;
        let namespace = lock_namespace(&mut tx, input.namespace)
            .await?
            .ok_or_else(|| {
                Error::not_found("cache_namespace", "id", input.namespace.to_string())
            })?;
        ensure_namespace_writable(&namespace)?;

        let existing_query = format!(
            "SELECT {CACHE_GENERATION_SELECT_COLUMNS} FROM cache_generation \
             WHERE namespace = $1 AND client_refresh_id = $2 FOR UPDATE"
        );
        if let Some(existing) = sqlx::query_as::<_, CacheGeneration>(&existing_query)
            .bind(input.namespace)
            .bind(&input.client_refresh_id)
            .fetch_optional(&mut *tx)
            .await?
        {
            if generation_matches_input(&existing, input) {
                tx.commit().await?;
                return Ok(CreateCacheGenerationResult::Existing(existing));
            }
            return Err(Error::already_exists(
                "cache_generation",
                "client_refresh_id",
                input.client_refresh_id.clone(),
            ));
        }

        let staging_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cache_generation \
             WHERE namespace = $1 AND state IN ('staging', 'ready')",
        )
        .bind(input.namespace)
        .fetch_one(&mut *tx)
        .await?;
        if staging_count >= i64::from(namespace.max_staging_generations) {
            return Err(Error::validation(
                "cache namespace staging generation quota exceeded",
            ));
        }
        ensure_generation_admission(&mut tx, &namespace, input.expected_bytes, admission).await?;

        let insert = format!(
            "INSERT INTO cache_generation \
             (namespace, client_refresh_id, expected_active_generation, expected_chunk_count, \
              expected_count, expected_bytes, checksum_algorithm, checksum, source_revision, \
              created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             RETURNING {CACHE_GENERATION_SELECT_COLUMNS}"
        );
        let generation = sqlx::query_as::<_, CacheGeneration>(&insert)
            .bind(input.namespace)
            .bind(&input.client_refresh_id)
            .bind(input.expected_active_generation)
            .bind(input.expected_chunk_count)
            .bind(input.expected_count)
            .bind(input.expected_bytes)
            .bind(&input.checksum_algorithm)
            .bind(&input.checksum)
            .bind(&input.source_revision)
            .bind(input.created_by)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(CreateCacheGenerationResult::Created(generation))
    }

    pub async fn list_for_namespace(
        pool: &PgPool,
        namespace_id: Id,
        limit: i64,
    ) -> Result<Vec<CacheGeneration>> {
        Ok(
            Self::list_for_namespace_page(pool, namespace_id, None, limit)
                .await?
                .items,
        )
    }

    /// Reverse-chronological keyset page. `before` is the final `(created,id)`
    /// pair from the preceding page.
    pub async fn list_for_namespace_page(
        pool: &PgPool,
        namespace_id: Id,
        before: Option<(DateTime<Utc>, Id)>,
        limit: i64,
    ) -> Result<CacheGenerationPage> {
        let limit = bounded_limit(limit, MAX_CLEANUP_SELECTION, "generation list")?;
        let fetch_limit = limit + 1;
        let (before_created, before_id) = before.unzip();
        let query = format!(
            "SELECT {CACHE_GENERATION_SELECT_COLUMNS} FROM cache_generation \
             WHERE namespace = $1 \
               AND ($2::TIMESTAMPTZ IS NULL \
                    OR (created, id) < ($2::TIMESTAMPTZ, $3::BIGINT)) \
             ORDER BY created DESC, id DESC LIMIT $4"
        );
        let mut items = sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(namespace_id)
            .bind(before_created)
            .bind(before_id)
            .bind(fetch_limit)
            .fetch_all(pool)
            .await?;
        let has_more = items.len() > limit as usize;
        if has_more {
            items.pop();
        }
        let next_before = has_more
            .then(|| items.last().map(|item| (item.created, item.id)))
            .flatten();
        Ok(CacheGenerationPage { items, next_before })
    }

    /// Selects a bounded oldest-first batch of abandoned unpublished
    /// generations without letting generations in other states consume the
    /// selection window.
    pub async fn select_expired_unpublished(
        pool: &PgPool,
        namespace_id: Id,
        created_before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<CacheGeneration>> {
        let limit = bounded_limit(
            limit,
            MAX_CLEANUP_SELECTION,
            "expired unpublished selection",
        )?;
        let query = format!(
            "SELECT {CACHE_GENERATION_SELECT_COLUMNS} FROM cache_generation \
             WHERE namespace = $1 AND state IN ('staging', 'ready') AND created < $2 \
             ORDER BY created, id LIMIT $3"
        );
        sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(namespace_id)
            .bind(created_before)
            .bind(limit)
            .fetch_all(pool)
            .await
            .map_err(Into::into)
    }

    /// Validates a complete staging generation and makes it ready for
    /// publication. Once ready, entry writes are rejected by both repository
    /// locking and the database trigger.
    pub async fn seal(pool: &PgPool, generation_id: Id) -> Result<CacheGeneration> {
        Self::seal_with_expectations(pool, generation_id, None).await
    }

    /// Seals a staging generation while validating caller-supplied completion
    /// expectations in the same transaction as the `staging -> ready`
    /// transition. This prevents a concurrent promoter from observing a ready
    /// generation before seal-time count/byte validation completes.
    pub async fn seal_with_expectations(
        pool: &PgPool,
        generation_id: Id,
        requested: Option<SealCacheGenerationInput>,
    ) -> Result<CacheGeneration> {
        let mut tx = pool.begin().await?;
        // Resolve the (immutable) owning namespace first so locks are always
        // acquired namespace-before-generation, matching create/promote/
        // tombstone and avoiding tombstone-vs-seal AB/BA deadlocks.
        let namespace_id = generation_namespace(&mut tx, generation_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_generation", "id", generation_id.to_string()))?;
        let namespace = lock_namespace(&mut tx, namespace_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_namespace", "id", namespace_id.to_string()))?;
        let generation = lock_generation(&mut tx, generation_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_generation", "id", generation_id.to_string()))?;
        ensure_namespace_writable(&namespace)?;

        let expectations = requested.unwrap_or(SealCacheGenerationInput {
            expected_chunk_count: generation.expected_chunk_count,
            expected_count: generation.expected_count,
            expected_bytes: generation.expected_bytes,
        });
        if expectations.expected_chunk_count != generation.expected_chunk_count {
            return Err(Error::invalid_state(
                "expected_chunk_count does not match the staging generation",
            ));
        }
        if let (Some(created), Some(sealed)) =
            (generation.expected_count, expectations.expected_count)
        {
            if created != sealed {
                return Err(Error::invalid_state(
                    "expected_record_count does not match the staging generation",
                ));
            }
        }
        if generation.state == CacheGenerationState::Ready {
            if expectations
                .expected_count
                .is_some_and(|count| count != generation.record_count)
                || expectations
                    .expected_bytes
                    .is_some_and(|bytes| bytes != generation.size_bytes)
            {
                return Err(Error::invalid_state(
                    "seal replay expectations do not match the ready generation",
                ));
            }
            tx.commit().await?;
            return Ok(generation);
        }
        if generation.state != CacheGenerationState::Staging {
            return Err(Error::invalid_state(
                "only staging or matching ready cache generations may be sealed",
            ));
        }
        if let (Some(created), Some(sealed)) =
            (generation.expected_bytes, expectations.expected_bytes)
        {
            if created != sealed {
                return Err(Error::invalid_state(
                    "expected_size_bytes does not match the staging generation",
                ));
            }
        }

        let (chunk_count, min_chunk, max_chunk, chunk_record_count, chunk_size_bytes): (
            i64,
            Option<i32>,
            Option<i32>,
            i64,
            i64,
        ) = sqlx::query_as(
            "SELECT COUNT(*), MIN(chunk_index), MAX(chunk_index), \
                    COALESCE(SUM(record_count), 0)::BIGINT, COALESCE(SUM(size_bytes), 0)::BIGINT \
             FROM cache_ingest_chunk WHERE generation = $1",
        )
        .bind(generation_id)
        .fetch_one(&mut *tx)
        .await?;
        let chunks_are_contiguous = if generation.expected_chunk_count == 0 {
            chunk_count == 0
        } else {
            chunk_count == i64::from(generation.expected_chunk_count)
                && min_chunk == Some(0)
                && max_chunk == Some(generation.expected_chunk_count - 1)
        };
        if !chunks_are_contiguous {
            return Err(Error::validation(
                "cache generation ingest chunks must be contiguous from zero through expected_chunk_count - 1",
            ));
        }

        let (record_count, size_bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0)::BIGINT FROM cache_entry WHERE generation = $1",
        )
        .bind(generation_id)
        .fetch_one(&mut *tx)
        .await?;
        if record_count != chunk_record_count || size_bytes != chunk_size_bytes {
            return Err(Error::validation(
                "cache generation entries do not match accepted ingest chunk metadata",
            ));
        }
        if generation
            .expected_count
            .is_some_and(|count| count != record_count)
        {
            return Err(Error::validation(
                "cache generation record count does not match expected_count",
            ));
        }
        if generation
            .expected_bytes
            .is_some_and(|bytes| bytes != size_bytes)
        {
            return Err(Error::validation(
                "cache generation byte count does not match expected_bytes",
            ));
        }
        if expectations
            .expected_count
            .is_some_and(|count| count != record_count)
        {
            return Err(Error::invalid_state(
                "sealed record count did not match expected_record_count",
            ));
        }
        if expectations
            .expected_bytes
            .is_some_and(|bytes| bytes != size_bytes)
        {
            return Err(Error::invalid_state(
                "sealed size did not match expected_size_bytes",
            ));
        }
        ensure_generation_quota(&namespace, record_count, size_bytes)?;

        let query = format!(
            "UPDATE cache_generation SET state = 'ready', record_count = $2, size_bytes = $3, \
             sealed = NOW() WHERE id = $1 AND state = 'staging' \
             RETURNING {CACHE_GENERATION_SELECT_COLUMNS}"
        );
        let sealed = sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(generation_id)
            .bind(record_count)
            .bind(size_bytes)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| Error::invalid_state("cache generation changed while sealing"))?;
        tx.commit().await?;
        Ok(sealed)
    }

    /// Promotes a ready generation using the observed active-generation value
    /// as an optimistic concurrency guard. `None` explicitly means first
    /// publication.
    pub async fn promote(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
        expected_active_generation: Option<Id>,
        prior_readable_until: DateTime<Utc>,
    ) -> Result<CachePromotionResult> {
        let mut tx = pool.begin().await?;
        let namespace = lock_namespace(&mut tx, namespace_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_namespace", "id", namespace_id.to_string()))?;
        ensure_namespace_writable(&namespace)?;

        if namespace.active_generation == Some(generation_id) {
            let generation = lock_generation(&mut tx, generation_id)
                .await?
                .ok_or_else(|| {
                    Error::not_found("cache_generation", "id", generation_id.to_string())
                })?;
            if generation.namespace == namespace_id
                && generation.state == CacheGenerationState::Active
                && generation.expected_active_generation == expected_active_generation
            {
                tx.commit().await?;
                return Ok(CachePromotionResult {
                    namespace,
                    activated_generation: generation,
                    retired_generation: expected_active_generation,
                    replayed: true,
                });
            }
        }

        if namespace.active_generation != expected_active_generation {
            return Err(Error::invalid_state(
                "cache namespace active generation changed before promotion",
            ));
        }

        let generation = lock_generation(&mut tx, generation_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_generation", "id", generation_id.to_string()))?;
        if generation.namespace != namespace_id || generation.state != CacheGenerationState::Ready {
            return Err(Error::invalid_state(
                "only a ready generation belonging to the namespace may be promoted",
            ));
        }
        if generation.expected_active_generation != expected_active_generation {
            return Err(Error::invalid_state(
                "promotion expected_active_generation_id does not match the staging generation",
            ));
        }

        let (retained_count, aggregate_bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*) FILTER (WHERE state IN ('active', 'retired')), \
                    COALESCE(SUM(size_bytes), 0)::BIGINT \
             FROM cache_generation WHERE namespace = $1",
        )
        .bind(namespace_id)
        .fetch_one(&mut *tx)
        .await?;
        if retained_count + 1 > i64::from(namespace.max_retained_generations)
            || aggregate_bytes > namespace.max_retained_bytes
        {
            return Err(Error::validation(
                "cache namespace retained generation quota would be exceeded",
            ));
        }

        let retired_generation = namespace.active_generation;
        if let Some(previous_id) = retired_generation {
            let retired = sqlx::query(
                "UPDATE cache_generation SET state = 'retired', retired = NOW(), \
                 readable_until = GREATEST(COALESCE(readable_until, $2), $2) \
                 WHERE id = $1 AND state = 'active'",
            )
            .bind(previous_id)
            .bind(prior_readable_until)
            .execute(&mut *tx)
            .await?;
            if retired.rows_affected() != 1 {
                return Err(Error::invalid_state(
                    "namespace active generation pointer and generation state disagree",
                ));
            }
        }

        let activate = format!(
            "UPDATE cache_generation SET state = 'active', activated = NOW() \
             WHERE id = $1 AND state = 'ready' RETURNING {CACHE_GENERATION_SELECT_COLUMNS}"
        );
        let activated_generation = sqlx::query_as::<_, CacheGeneration>(&activate)
            .bind(generation_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| Error::invalid_state("cache generation changed while promoting"))?;

        let namespace_query = format!(
            "UPDATE cache_namespace SET active_generation = $2, \
             consecutive_refresh_failures = 0, last_refresh_failure_at = NULL WHERE id = $1 \
             RETURNING {CACHE_NAMESPACE_SELECT_COLUMNS}"
        );
        let namespace = sqlx::query_as::<_, CacheNamespace>(&namespace_query)
            .bind(namespace_id)
            .bind(generation_id)
            .fetch_one(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(CachePromotionResult {
            namespace,
            activated_generation,
            retired_generation,
            replayed: false,
        })
    }

    pub async fn fail(pool: &PgPool, generation_id: Id, reason: &str) -> Result<CacheGeneration> {
        validate_required_text(
            reason,
            MAX_CACHE_REASON_BYTES,
            "cache generation failure reason",
        )?;
        let mut tx = pool.begin().await?;
        let namespace_id = generation_namespace(&mut tx, generation_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_generation", "id", generation_id.to_string()))?;
        lock_namespace(&mut tx, namespace_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_namespace", "id", namespace_id.to_string()))?;
        let existing = lock_generation(&mut tx, generation_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_generation", "id", generation_id.to_string()))?;
        if existing.state == CacheGenerationState::Failed {
            if existing.failure_reason.as_deref() == Some(reason) {
                tx.commit().await?;
                return Ok(existing);
            }
            return Err(Error::invalid_state(
                "cache generation is already failed with a different reason",
            ));
        }

        let query = format!(
            "UPDATE cache_generation SET state = 'failed', failed = NOW(), failure_reason = $2 \
             WHERE id = $1 AND state IN ('staging', 'ready') \
             RETURNING {CACHE_GENERATION_SELECT_COLUMNS}"
        );
        let failed = sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(generation_id)
            .bind(reason)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| Error::invalid_state("only staging or ready generations may fail"))?;
        sqlx::query(
            "UPDATE cache_namespace \
             SET consecutive_refresh_failures = LEAST(consecutive_refresh_failures::BIGINT + 1, 2147483647)::INTEGER, \
                 last_refresh_failure_at = NOW() WHERE id = $1",
        )
        .bind(namespace_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(failed)
    }

    /// Returns a generation only when it is a readable active snapshot or an
    /// unexpired retired snapshot belonging to this namespace.
    pub async fn find_readable_pinned(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
    ) -> Result<Option<CacheGeneration>> {
        let query = format!(
            "SELECT {} FROM cache_generation g \
             JOIN cache_namespace n ON n.id = g.namespace \
             WHERE n.id = $1 AND g.id = $2 AND n.tombstoned_at IS NULL \
             AND (g.state = 'active' OR (g.state = 'retired' AND g.readable_until > NOW()))",
            qualified_columns("g", CACHE_GENERATION_SELECT_COLUMNS),
        );
        sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(namespace_id)
            .bind(generation_id)
            .fetch_optional(pool)
            .await
            .map_err(Into::into)
    }

    /// Selects expired, non-active generations for bounded cleanup work.
    pub async fn select_cleanup_candidates(
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<CacheGeneration>> {
        let limit = bounded_limit(limit, MAX_CLEANUP_SELECTION, "cleanup selection")?;
        let query = format!(
            "SELECT {CACHE_GENERATION_SELECT_COLUMNS} FROM cache_generation g \
             WHERE (state = 'failed' \
                OR (state = 'retired' AND readable_until IS NOT NULL AND readable_until <= NOW())) \
               AND NOT EXISTS (SELECT 1 FROM workflow_cache_iteration i \
                               JOIN workflow_execution w ON w.id = i.workflow_execution \
                               WHERE i.generation = g.id AND i.state = 'scanning' \
                                 AND w.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')) \
             ORDER BY COALESCE(readable_until, failed, retired, created), id LIMIT $1"
        );
        sqlx::query_as::<_, CacheGeneration>(&query)
            .bind(limit)
            .fetch_all(pool)
            .await
            .map_err(Into::into)
    }

    /// Removes an empty, non-active generation after its entries were removed
    /// by [`CacheEntryRepository::delete_cleanup_batch`]. The subordinate
    /// ingest-chunk metadata is dropped in the same transaction so the
    /// `ON DELETE RESTRICT` foreign key does not block finalization.
    pub async fn delete_if_empty(pool: &PgPool, generation_id: Id) -> Result<bool> {
        let mut tx = pool.begin().await?;
        let eligible: Option<Id> = sqlx::query_scalar(
            "SELECT g.id FROM cache_generation g WHERE g.id = $1 \
             AND (g.state = 'failed' \
                  OR (g.state = 'retired' AND g.readable_until IS NOT NULL \
                      AND g.readable_until <= NOW())) \
             AND NOT EXISTS (SELECT 1 FROM cache_entry e WHERE e.generation = g.id) \
              AND NOT EXISTS (SELECT 1 FROM workflow_cache_iteration i \
                              JOIN workflow_execution w ON w.id = i.workflow_execution \
                              WHERE i.generation = g.id AND i.state = 'scanning' \
                                AND w.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')) \
             FOR UPDATE",
        )
        .bind(generation_id)
        .fetch_optional(&mut *tx)
        .await?;
        if eligible.is_none() {
            tx.rollback().await?;
            return Ok(false);
        }

        sqlx::query("DELETE FROM cache_ingest_chunk WHERE generation = $1")
            .bind(generation_id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM cache_generation WHERE id = $1")
            .bind(generation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }
}

pub struct CacheIngestRepository;

impl CacheIngestRepository {
    /// Atomically accepts an ingest chunk. The same accepted checksum is a
    /// no-op replay; a distinct checksum for that chunk index is a conflict.
    pub async fn insert_chunk(
        pool: &PgPool,
        generation_id: Id,
        chunk_index: i32,
        request_checksum: &str,
        entries: &[CacheEntryInput],
    ) -> Result<InsertCacheChunkResult> {
        Self::insert_chunk_with_policy(
            pool,
            generation_id,
            chunk_index,
            request_checksum,
            entries,
            &CacheAdmissionConfig::default(),
        )
        .await
    }

    pub async fn insert_chunk_with_policy(
        pool: &PgPool,
        generation_id: Id,
        chunk_index: i32,
        request_checksum: &str,
        entries: &[CacheEntryInput],
        admission: &CacheAdmissionConfig,
    ) -> Result<InsertCacheChunkResult> {
        validate_chunk_input(chunk_index, request_checksum, entries)?;
        let mut tx = pool.begin().await?;
        lock_cache_admission(&mut tx).await?;
        // Lock namespace-before-generation (the generation's namespace is
        // immutable) so tombstone, upload, and seal share one lock order and
        // cannot deadlock against each other.
        let namespace_id = generation_namespace(&mut tx, generation_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_generation", "id", generation_id.to_string()))?;
        let namespace = lock_namespace(&mut tx, namespace_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_namespace", "id", namespace_id.to_string()))?;
        let generation = lock_generation(&mut tx, generation_id)
            .await?
            .ok_or_else(|| Error::not_found("cache_generation", "id", generation_id.to_string()))?;
        ensure_namespace_writable(&namespace)?;
        if chunk_index >= generation.expected_chunk_count {
            return Err(Error::validation(
                "cache ingest chunk index exceeds expected_chunk_count",
            ));
        }

        let existing_query = format!(
            "SELECT {CACHE_INGEST_CHUNK_SELECT_COLUMNS} FROM cache_ingest_chunk \
             WHERE generation = $1 AND chunk_index = $2"
        );
        if let Some(existing) = sqlx::query_as::<_, CacheIngestChunk>(&existing_query)
            .bind(generation_id)
            .bind(chunk_index)
            .fetch_optional(&mut *tx)
            .await?
        {
            if existing.request_checksum == request_checksum {
                tx.commit().await?;
                return Ok(InsertCacheChunkResult::Replayed(existing));
            }
            return Err(Error::already_exists(
                "cache_ingest_chunk",
                "generation/chunk_index",
                format!("{generation_id}/{chunk_index}"),
            ));
        }

        if generation.state != CacheGenerationState::Staging {
            return Err(Error::invalid_state(
                "cache entries may only be written to staging generations",
            ));
        }

        let entry_count = i64::try_from(entries.len())
            .map_err(|_| Error::validation("cache ingest chunk is too large"))?;
        if generation.record_count.saturating_add(entry_count)
            > namespace.max_records_per_generation
        {
            return Err(Error::validation(
                "cache generation record quota would be exceeded",
            ));
        }

        let mut inserted_bytes = 0_i64;
        for batch in entries.chunks(INGEST_INSERT_BATCH_SIZE) {
            let mut query = QueryBuilder::<Postgres>::new(
                "INSERT INTO cache_entry \
                 (generation, external_id, value, source_updated_at, source_checksum) ",
            );
            query.push_values(batch, |mut row, entry| {
                row.push_bind(generation_id)
                    .push_bind(&entry.external_id)
                    .push_bind(&entry.value)
                    .push_bind(entry.source_updated_at)
                    .push_bind(entry.source_checksum.as_deref());
            });
            query.push(" RETURNING size_bytes");
            let inserted: Vec<i64> = match query.build_query_scalar().fetch_all(&mut *tx).await {
                Ok(rows) => rows,
                // The only unique constraint reachable by a cache_entry insert
                // is the (generation, external_id) index, so a 23505 here means
                // the chunk (or a prior chunk in this generation) repeats an
                // external identifier. Surface a typed, ID-free ingestion error.
                Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                    return Err(Error::cache_duplicate_external_id());
                }
                Err(err) => return Err(err.into()),
            };
            inserted_bytes = inserted_bytes
                .checked_add(inserted.into_iter().sum::<i64>())
                .ok_or_else(|| Error::validation("cache generation byte size overflow"))?;
        }

        let next_size = generation
            .size_bytes
            .checked_add(inserted_bytes)
            .ok_or_else(|| Error::validation("cache generation byte size overflow"))?;
        ensure_generation_quota(
            &namespace,
            generation.record_count.saturating_add(entry_count),
            next_size,
        )?;
        let other_generation_bytes: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(size_bytes), 0)::BIGINT FROM cache_generation \
             WHERE namespace = $1 AND id <> $2",
        )
        .bind(namespace_id)
        .bind(generation_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure_namespace_storage_quota(
            &namespace,
            other_generation_bytes.saturating_add(next_size),
        )?;
        ensure_physical_byte_admission(&mut tx, &namespace, admission).await?;

        let chunk_query = format!(
            "INSERT INTO cache_ingest_chunk (generation, chunk_index, request_checksum, record_count, size_bytes) \
             VALUES ($1, $2, $3, $4, $5) RETURNING {CACHE_INGEST_CHUNK_SELECT_COLUMNS}"
        );
        let chunk = sqlx::query_as::<_, CacheIngestChunk>(&chunk_query)
            .bind(generation_id)
            .bind(chunk_index)
            .bind(request_checksum)
            .bind(entry_count)
            .bind(inserted_bytes)
            .fetch_one(&mut *tx)
            .await?;

        sqlx::query(
            "UPDATE cache_generation SET record_count = $2, size_bytes = $3 \
             WHERE id = $1 AND state = 'staging'",
        )
        .bind(generation_id)
        .bind(generation.record_count.saturating_add(entry_count))
        .bind(next_size)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(InsertCacheChunkResult::Inserted(chunk))
    }

    pub async fn list_chunks(pool: &PgPool, generation_id: Id) -> Result<Vec<CacheIngestChunk>> {
        let query = format!(
            "SELECT {CACHE_INGEST_CHUNK_SELECT_COLUMNS} FROM cache_ingest_chunk \
             WHERE generation = $1 ORDER BY chunk_index"
        );
        sqlx::query_as::<_, CacheIngestChunk>(&query)
            .bind(generation_id)
            .fetch_all(pool)
            .await
            .map_err(Into::into)
    }
}

pub struct CacheEntryRepository;

impl CacheEntryRepository {
    pub async fn find_active(
        pool: &PgPool,
        namespace_id: Id,
        external_id: &str,
    ) -> Result<Option<CacheEntry>> {
        validate_external_id(external_id)?;
        let query = format!(
            "SELECT {} FROM cache_entry e \
             JOIN cache_namespace n ON n.active_generation = e.generation \
             JOIN cache_generation g ON g.id = e.generation \
             WHERE n.id = $1 AND n.tombstoned_at IS NULL AND g.state = 'active' \
             AND e.external_id = $2",
            qualified_columns("e", CACHE_ENTRY_SELECT_COLUMNS),
        );
        sqlx::query_as::<_, CacheEntry>(&query)
            .bind(namespace_id)
            .bind(external_id)
            .fetch_optional(pool)
            .await
            .map_err(Into::into)
    }

    /// Returns found records in the supplied ID order. Missing IDs are omitted
    /// so callers can identify them from their bounded request set.
    pub async fn find_active_many(
        pool: &PgPool,
        namespace_id: Id,
        external_ids: &[String],
    ) -> Result<Vec<CacheEntry>> {
        let external_ids = deduplicate_lookup_ids(external_ids)?;
        if external_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = pool.begin().await?;
        let active_generation: Option<Id> = sqlx::query_scalar(
            "SELECT active_generation FROM cache_namespace \
             WHERE id = $1 AND tombstoned_at IS NULL FOR SHARE",
        )
        .bind(namespace_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let Some(active_generation) = active_generation else {
            tx.commit().await?;
            return Ok(Vec::new());
        };
        let response_bytes: i64 = sqlx::query_scalar(
            "WITH requested AS ( \
                 SELECT external_id, MIN(ordinal) AS ordinal \
                 FROM unnest($2::TEXT[]) WITH ORDINALITY AS input(external_id, ordinal) \
                 GROUP BY external_id \
             ) \
             SELECT COALESCE(SUM(octet_length(e.value::TEXT) \
                 + octet_length(e.external_id) \
                 + COALESCE(octet_length(e.source_checksum), 0) + 256), 0)::BIGINT \
             FROM requested \
             JOIN cache_entry e ON e.generation = $1 \
                AND e.external_id = requested.external_id",
        )
        .bind(active_generation)
        .bind(&external_ids)
        .fetch_one(&mut *tx)
        .await?;
        ensure_read_budget(
            response_bytes,
            MAX_MULTI_LOOKUP_BYTES,
            "cache multi-ID lookup",
        )?;

        let query = format!(
            "WITH requested AS ( \
                 SELECT external_id, MIN(ordinal) AS ordinal \
                 FROM unnest($2::TEXT[]) WITH ORDINALITY AS input(external_id, ordinal) \
                 GROUP BY external_id \
             ) \
             SELECT {} FROM requested \
             JOIN cache_entry e ON e.generation = $1 \
                AND e.external_id = requested.external_id \
             ORDER BY requested.ordinal",
            qualified_columns("e", CACHE_ENTRY_SELECT_COLUMNS),
        );
        let entries = sqlx::query_as::<_, CacheEntry>(&query)
            .bind(active_generation)
            .bind(&external_ids)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(entries)
    }

    /// Finds one entry in a readable active or unexpired retired generation.
    /// The generation is share-locked through the lookup so cleanup or
    /// retirement cannot invalidate the snapshot between validation and read.
    pub async fn find_pinned(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
        external_id: &str,
    ) -> Result<Option<CacheEntry>> {
        validate_external_id(external_id)?;
        let mut tx = pool.begin().await?;
        load_readable_pinned(&mut tx, namespace_id, generation_id).await?;
        let query = format!(
            "SELECT {CACHE_ENTRY_SELECT_COLUMNS} FROM cache_entry \
             WHERE generation = $1 AND external_id = $2"
        );
        let entry = sqlx::query_as::<_, CacheEntry>(&query)
            .bind(generation_id)
            .bind(external_id)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(entry)
    }

    /// Finds a bounded set of entries in one readable pinned generation,
    /// preserving the caller's requested order.
    pub async fn find_pinned_many(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
        external_ids: &[String],
    ) -> Result<Vec<CacheEntry>> {
        let external_ids = deduplicate_lookup_ids(external_ids)?;
        if external_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = pool.begin().await?;
        load_readable_pinned(&mut tx, namespace_id, generation_id).await?;
        let response_bytes: i64 = sqlx::query_scalar(
            "WITH requested AS ( \
                 SELECT external_id, MIN(ordinal) AS ordinal \
                 FROM unnest($2::TEXT[]) WITH ORDINALITY AS input(external_id, ordinal) \
                 GROUP BY external_id \
             ) \
             SELECT COALESCE(SUM(octet_length(e.value::TEXT) \
                 + octet_length(e.external_id) \
                 + COALESCE(octet_length(e.source_checksum), 0) + 256), 0)::BIGINT \
             FROM requested \
             JOIN cache_entry e ON e.generation = $1 \
                AND e.external_id = requested.external_id",
        )
        .bind(generation_id)
        .bind(&external_ids)
        .fetch_one(&mut *tx)
        .await?;
        ensure_read_budget(
            response_bytes,
            MAX_MULTI_LOOKUP_BYTES,
            "cache multi-ID lookup",
        )?;
        let query = format!(
            "WITH requested AS ( \
                 SELECT external_id, MIN(ordinal) AS ordinal \
                 FROM unnest($2::TEXT[]) WITH ORDINALITY AS input(external_id, ordinal) \
                 GROUP BY external_id \
             ) \
             SELECT {} FROM requested \
             JOIN cache_entry e ON e.generation = $1 \
                AND e.external_id = requested.external_id \
             ORDER BY requested.ordinal",
            qualified_columns("e", CACHE_ENTRY_SELECT_COLUMNS),
        );
        let entries = sqlx::query_as::<_, CacheEntry>(&query)
            .bind(generation_id)
            .bind(&external_ids)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(entries)
    }

    /// Keyset scan of a generation-pinned snapshot. `after_external_id` is
    /// compared using the same C collation as the unique entry index.
    ///
    /// The readability check and the scan run in one transaction with the
    /// generation row share-locked, so the snapshot cannot be retired,
    /// expired, or removed between the two statements. A generation that is
    /// expired, removed, tombstoned, or in another namespace yields a distinct
    /// [`Error::CacheSnapshotExpired`]; an empty `Vec` is returned only for a
    /// genuine end-of-generation page.
    pub async fn scan_pinned(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
        after_external_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<CacheEntry>> {
        Self::scan_pinned_with_generation(
            pool,
            namespace_id,
            generation_id,
            after_external_id,
            limit,
        )
        .await
        .map(|(_, entries)| entries)
    }

    /// Returns generation metadata from the same share-locked transaction as
    /// the page rows so cursor expiration/count metadata cannot race cleanup.
    pub async fn scan_pinned_with_generation(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
        after_external_id: Option<&str>,
        limit: i64,
    ) -> Result<(CacheGeneration, Vec<CacheEntry>)> {
        let limit = bounded_limit(limit, MAX_SCAN_PAGE_SIZE, "cache scan")?;
        if let Some(external_id) = after_external_id {
            validate_external_id(external_id)?;
        }

        let page =
            Self::scan_pinned_page(pool, namespace_id, generation_id, after_external_id, limit)
                .await?;
        Ok((page.generation, page.entries))
    }

    /// Byte-bounded keyset page with an explicit continuation signal.
    pub async fn scan_pinned_page(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
        after_external_id: Option<&str>,
        limit: i64,
    ) -> Result<CacheEntryPage> {
        Self::scan_pinned_page_with_budget(
            pool,
            namespace_id,
            generation_id,
            after_external_id,
            limit,
            MAX_SCAN_MATERIALIZATION_BYTES,
        )
        .await
    }

    /// Byte-bounded keyset page using a caller-selected response budget that
    /// cannot exceed the repository materialization ceiling.
    pub async fn scan_pinned_page_with_budget(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
        after_external_id: Option<&str>,
        limit: i64,
        max_materialization_bytes: i64,
    ) -> Result<CacheEntryPage> {
        let limit = bounded_limit(limit, MAX_SCAN_PAGE_SIZE, "cache scan")?;
        let max_materialization_bytes = bounded_limit(
            max_materialization_bytes,
            MAX_SCAN_MATERIALIZATION_BYTES,
            "cache scan byte budget",
        )?;
        if let Some(external_id) = after_external_id {
            validate_external_id(external_id)?;
        }

        let mut tx = pool.begin().await?;
        let generation = load_readable_pinned(&mut tx, namespace_id, generation_id).await?;
        let query = format!(
            "WITH candidates AS MATERIALIZED ( \
                 SELECT id, external_id, \
                        (octet_length(value::TEXT) + octet_length(external_id) \
                         + COALESCE(octet_length(source_checksum), 0) + 256)::BIGINT \
                            AS response_bytes \
                 FROM cache_entry \
                 WHERE generation = $1 \
                   AND ($2::TEXT IS NULL \
                        OR external_id COLLATE \"C\" > $2::TEXT COLLATE \"C\") \
                 ORDER BY external_id COLLATE \"C\" ASC LIMIT $3 \
             ), bounded AS ( \
                 SELECT id, external_id, \
                        SUM(response_bytes) OVER ( \
                            ORDER BY external_id COLLATE \"C\", id) AS running_bytes, \
                        ROW_NUMBER() OVER ( \
                            ORDER BY external_id COLLATE \"C\", id) AS row_number \
                 FROM candidates \
             ) \
             SELECT {} FROM bounded b \
             JOIN cache_entry e ON e.id = b.id \
             WHERE b.running_bytes <= $4 OR b.row_number = 1 \
             ORDER BY b.external_id COLLATE \"C\", b.id",
            qualified_columns("e", CACHE_ENTRY_SELECT_COLUMNS),
        );
        let entries = sqlx::query_as::<_, CacheEntry>(&query)
            .bind(generation_id)
            .bind(after_external_id)
            .bind(limit)
            .bind(max_materialization_bytes)
            .fetch_all(&mut *tx)
            .await?;
        let has_more = if let Some(last) = entries.last() {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS( \
                    SELECT 1 FROM cache_entry \
                    WHERE generation = $1 \
                      AND external_id COLLATE \"C\" > $2::TEXT COLLATE \"C\")",
            )
            .bind(generation_id)
            .bind(&last.external_id)
            .fetch_one(&mut *tx)
            .await?
        } else {
            false
        };
        tx.commit().await?;
        Ok(CacheEntryPage {
            generation,
            entries,
            has_more,
        })
    }

    /// Transaction-scoped variant used by orchestration code that must persist
    /// its durable cursor atomically with the materialized page.
    pub async fn scan_pinned_page_with_conn(
        conn: &mut sqlx::PgConnection,
        namespace_id: Id,
        generation_id: Id,
        after_external_id: Option<&str>,
        limit: i64,
    ) -> Result<CacheEntryPage> {
        Self::scan_pinned_page_with_budget_conn(
            conn,
            namespace_id,
            generation_id,
            after_external_id,
            limit,
            MAX_SCAN_MATERIALIZATION_BYTES,
        )
        .await
    }

    /// Transaction-scoped scan with a caller-selected remaining materialization budget.
    pub async fn scan_pinned_page_with_budget_conn(
        conn: &mut sqlx::PgConnection,
        namespace_id: Id,
        generation_id: Id,
        after_external_id: Option<&str>,
        limit: i64,
        max_materialization_bytes: i64,
    ) -> Result<CacheEntryPage> {
        let limit = bounded_limit(limit, MAX_SCAN_PAGE_SIZE, "cache scan")?;
        let max_materialization_bytes = bounded_limit(
            max_materialization_bytes,
            MAX_SCAN_MATERIALIZATION_BYTES,
            "cache scan byte budget",
        )?;
        if let Some(external_id) = after_external_id {
            validate_external_id(external_id)?;
        }

        let generation = load_readable_pinned_conn(conn, namespace_id, generation_id).await?;
        let query = format!(
            "WITH candidates AS MATERIALIZED ( \
                 SELECT id, external_id, \
                        (octet_length(value::TEXT) + octet_length(external_id) \
                         + COALESCE(octet_length(source_checksum), 0) + 256)::BIGINT \
                            AS response_bytes \
                 FROM cache_entry \
                 WHERE generation = $1 \
                   AND ($2::TEXT IS NULL OR external_id COLLATE \"C\" > $2::TEXT COLLATE \"C\") \
                 ORDER BY external_id COLLATE \"C\" ASC LIMIT $3 \
             ), bounded AS ( \
                 SELECT id, external_id, \
                        SUM(response_bytes) OVER (ORDER BY external_id COLLATE \"C\", id) AS running_bytes, \
                        ROW_NUMBER() OVER (ORDER BY external_id COLLATE \"C\", id) AS row_number \
                 FROM candidates \
             ) \
             SELECT {} FROM bounded b \
             JOIN cache_entry e ON e.id = b.id \
             WHERE b.running_bytes <= $4 OR b.row_number = 1 \
             ORDER BY b.external_id COLLATE \"C\", b.id",
            qualified_columns("e", CACHE_ENTRY_SELECT_COLUMNS),
        );
        let entries = sqlx::query_as::<_, CacheEntry>(&query)
            .bind(generation_id)
            .bind(after_external_id)
            .bind(limit)
            .bind(max_materialization_bytes)
            .fetch_all(&mut *conn)
            .await?;
        let has_more = if let Some(last) = entries.last() {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM cache_entry \
                 WHERE generation = $1 AND external_id COLLATE \"C\" > $2 COLLATE \"C\")",
            )
            .bind(generation_id)
            .bind(&last.external_id)
            .fetch_one(&mut *conn)
            .await?
        } else {
            false
        };

        Ok(CacheEntryPage {
            generation,
            entries,
            has_more,
        })
    }

    /// Deletes at most one indexed batch from a cleanup candidate generation.
    /// It intentionally never deletes an entire high-cardinality generation in
    /// one transaction.
    pub async fn delete_cleanup_batch(pool: &PgPool, generation_id: Id, limit: i64) -> Result<u64> {
        let limit = bounded_limit(limit, MAX_CLEANUP_SELECTION, "cleanup batch")?;
        let mut tx = pool.begin().await?;
        let eligible: Option<Id> = sqlx::query_scalar(
            "SELECT id FROM cache_generation
             WHERE id = $1
               AND (state = 'failed'
                    OR (state = 'retired' AND readable_until IS NOT NULL
                        AND readable_until <= NOW()))
               AND NOT EXISTS (SELECT 1 FROM workflow_cache_iteration i
                               JOIN workflow_execution w ON w.id = i.workflow_execution
                               WHERE i.generation = cache_generation.id
                                 AND i.state = 'scanning'
                                 AND w.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned'))
             FOR UPDATE",
        )
        .bind(generation_id)
        .fetch_optional(&mut *tx)
        .await?;
        if eligible.is_none() {
            tx.rollback().await?;
            return Ok(0);
        }

        let result = sqlx::query(
            "WITH candidates AS ( \
                 SELECT e.id FROM cache_entry e \
                 WHERE e.generation = $1 \
                 ORDER BY e.id LIMIT $2 \
                 FOR UPDATE SKIP LOCKED \
             ) \
             DELETE FROM cache_entry e USING candidates c WHERE e.id = c.id",
        )
        .bind(generation_id)
        .bind(limit)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

async fn load_readable_pinned(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    namespace_id: Id,
    generation_id: Id,
) -> Result<CacheGeneration> {
    let query = format!(
        "SELECT {} FROM cache_generation g \
         JOIN cache_namespace n ON n.id = g.namespace \
         WHERE n.id = $1 AND g.id = $2 AND n.tombstoned_at IS NULL \
           AND (g.state = 'active' \
                OR (g.state = 'retired' AND (g.readable_until > NOW() \
                     OR EXISTS (SELECT 1 FROM workflow_cache_iteration i \
                                JOIN workflow_execution w ON w.id = i.workflow_execution \
                                WHERE i.generation = g.id AND i.state = 'scanning' \
                                   AND w.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned'))))) \
         FOR SHARE OF g",
        qualified_columns("g", CACHE_GENERATION_SELECT_COLUMNS),
    );
    let readable = sqlx::query_as::<_, CacheGeneration>(&query)
        .bind(namespace_id)
        .bind(generation_id)
        .fetch_optional(&mut **tx)
        .await?;
    readable.ok_or_else(|| {
        Error::cache_snapshot_expired(format!(
            "pinned cache generation {generation_id} is no longer readable in namespace {namespace_id}"
        ))
    })
}

async fn load_readable_pinned_conn(
    conn: &mut sqlx::PgConnection,
    namespace_id: Id,
    generation_id: Id,
) -> Result<CacheGeneration> {
    let query = format!(
        "SELECT {} FROM cache_generation g \
         JOIN cache_namespace n ON n.id = g.namespace \
         WHERE n.id = $1 AND g.id = $2 AND n.tombstoned_at IS NULL \
           AND (g.state = 'active' \
                OR (g.state = 'retired' AND (g.readable_until > NOW() \
                     OR EXISTS (SELECT 1 FROM workflow_cache_iteration i \
                                JOIN workflow_execution w ON w.id = i.workflow_execution \
                                WHERE i.generation = g.id AND i.state = 'scanning' \
                                   AND w.status NOT IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned'))))) \
         FOR SHARE OF g",
        qualified_columns("g", CACHE_GENERATION_SELECT_COLUMNS),
    );
    sqlx::query_as::<_, CacheGeneration>(&query)
        .bind(namespace_id)
        .bind(generation_id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| {
            Error::cache_snapshot_expired(format!(
                "pinned cache generation {generation_id} is no longer readable in namespace {namespace_id}"
            ))
        })
}

async fn insert_namespace<'e, E>(
    executor: E,
    input: &CreateCacheNamespaceInput,
    definition_ref: Option<&str>,
    managing_pack: Option<Id>,
    managing_pack_ref: Option<&str>,
) -> Result<CacheNamespace>
where
    E: Executor<'e, Database = Postgres> + 'e,
{
    let query = format!(
        "INSERT INTO cache_namespace \
         (owner_type, owner_identity, owner_pack, owner_pack_ref, owner_action, owner_action_ref, \
          owner_sensor, owner_sensor_ref, definition_ref, managing_pack, managing_pack_ref, \
          namespace, freshness_target_seconds, max_records_per_generation, max_generation_bytes, \
          max_retained_bytes, max_retained_generations, max_staging_generations) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) \
         RETURNING {CACHE_NAMESPACE_SELECT_COLUMNS}"
    );
    sqlx::query_as::<_, CacheNamespace>(&query)
        .bind(input.owner.owner_type)
        .bind(input.owner.owner_identity)
        .bind(input.owner.owner_pack)
        .bind(input.owner.owner_pack_ref.as_deref())
        .bind(input.owner.owner_action)
        .bind(input.owner.owner_action_ref.as_deref())
        .bind(input.owner.owner_sensor)
        .bind(input.owner.owner_sensor_ref.as_deref())
        .bind(definition_ref)
        .bind(managing_pack)
        .bind(managing_pack_ref)
        .bind(&input.namespace)
        .bind(input.policy.freshness_target_seconds)
        .bind(input.policy.max_records_per_generation)
        .bind(input.policy.max_generation_bytes)
        .bind(input.policy.max_retained_bytes)
        .bind(input.policy.max_retained_generations)
        .bind(input.policy.max_staging_generations)
        .fetch_one(executor)
        .await
        .map_err(Into::into)
}

async fn tombstone_namespace_in_transaction(
    connection: &mut sqlx::PgConnection,
    namespace_id: Id,
    reason: &str,
) -> Result<bool> {
    validate_tombstone_reason(reason)?;
    let namespace = lock_namespace(connection, namespace_id).await?;
    let Some(namespace) = namespace else {
        return Ok(false);
    };
    if namespace.tombstoned_at.is_some() {
        return Ok(true);
    }

    sqlx::query(
        "UPDATE cache_namespace SET tombstoned_at = NOW(), active_generation = NULL, \
         tombstone_reason = $2 WHERE id = $1",
    )
    .bind(namespace_id)
    .bind(reason)
    .execute(&mut *connection)
    .await?;

    sqlx::query(
        "UPDATE cache_generation SET state = 'failed', failed = NOW(), failure_reason = $2 \
         WHERE namespace = $1 AND state IN ('staging', 'ready')",
    )
    .bind(namespace_id)
    .bind(reason)
    .execute(&mut *connection)
    .await?;

    if let Some(active_generation) = namespace.active_generation {
        sqlx::query(
            "UPDATE cache_generation SET state = 'retired', retired = NOW(), \
             readable_until = NOW() WHERE id = $1 AND state = 'active'",
        )
        .bind(active_generation)
        .execute(&mut *connection)
        .await?;
    }

    Ok(true)
}

fn validate_managed_definition(definition: &ManagedCacheNamespaceDefinition) -> Result<()> {
    validate_required_text(
        &definition.definition_ref,
        MAX_CACHE_TEXT_BYTES,
        "managed cache definition ref",
    )?;
    if matches!(
        definition.owner.owner_type,
        OwnerType::System | OwnerType::Identity
    ) {
        return Err(Error::validation(
            "pack cache definitions support only pack, action, or sensor owners",
        ));
    }
    validate_namespace_name(&definition.namespace)?;
    definition.policy.validate()?;
    definition.owner.canonical_owner()?;
    Ok(())
}

fn validate_tombstone_reason(reason: &str) -> Result<()> {
    validate_required_text(
        reason,
        MAX_CACHE_REASON_BYTES,
        "cache namespace tombstone reason",
    )
}

fn namespace_policy(namespace: &CacheNamespace) -> CacheNamespacePolicy {
    CacheNamespacePolicy {
        freshness_target_seconds: namespace.freshness_target_seconds,
        max_records_per_generation: namespace.max_records_per_generation,
        max_generation_bytes: namespace.max_generation_bytes,
        max_retained_bytes: namespace.max_retained_bytes,
        max_retained_generations: namespace.max_retained_generations,
        max_staging_generations: namespace.max_staging_generations,
    }
}

pub(crate) fn validate_namespace_name(namespace: &str) -> Result<()> {
    let mut chars = namespace.chars();
    let Some(first) = chars.next() else {
        return Err(Error::validation("cache namespace cannot be empty"));
    };
    if namespace.len() > 128
        || !first.is_ascii_lowercase() && !first.is_ascii_digit()
        || !namespace.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-')
        })
    {
        return Err(Error::validation(
            "cache namespace must be lowercase ASCII [a-z0-9][a-z0-9._-]{0,127}",
        ));
    }
    Ok(())
}

fn validate_external_id(external_id: &str) -> Result<()> {
    if external_id.is_empty() || external_id.len() > 1024 {
        return Err(Error::validation(
            "cache external_id must be nonempty and no more than 1024 bytes",
        ));
    }
    Ok(())
}

fn deduplicate_lookup_ids(external_ids: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::with_capacity(external_ids.len().min(MAX_MULTI_LOOKUP_IDS));
    let mut deduplicated = Vec::with_capacity(external_ids.len().min(MAX_MULTI_LOOKUP_IDS));
    for external_id in external_ids {
        validate_external_id(external_id)?;
        if seen.insert(external_id.as_str()) {
            if deduplicated.len() == MAX_MULTI_LOOKUP_IDS {
                return Err(Error::validation(
                    "cache multi-ID lookup exceeds its maximum unique ID count",
                ));
            }
            deduplicated.push(external_id.clone());
        }
    }
    Ok(deduplicated)
}

fn validate_generation_input(input: &CreateCacheGenerationInput) -> Result<()> {
    if input.client_refresh_id.trim().is_empty()
        || input.expected_chunk_count < 0
        || input.expected_count.is_some_and(|value| value < 0)
        || input.expected_bytes.is_some_and(|value| value < 0)
    {
        return Err(Error::validation(
            "cache generation client refresh and expected values are invalid",
        ));
    }
    validate_required_text(
        &input.client_refresh_id,
        MAX_CACHE_TEXT_BYTES,
        "cache generation client_refresh_id",
    )?;
    validate_optional_text(
        input.source_revision.as_deref(),
        MAX_CACHE_TEXT_BYTES,
        "cache generation source_revision",
    )?;
    if input.checksum_algorithm.is_some() || input.checksum.is_some() {
        return Err(Error::validation(
            "whole-generation checksums are unsupported until canonical encoding is defined",
        ));
    }
    Ok(())
}

fn generation_matches_input(
    existing: &CacheGeneration,
    input: &CreateCacheGenerationInput,
) -> bool {
    existing.expected_active_generation == input.expected_active_generation
        && existing.expected_chunk_count == input.expected_chunk_count
        && existing.expected_count == input.expected_count
        && existing.expected_bytes == input.expected_bytes
        && existing.checksum_algorithm == input.checksum_algorithm
        && existing.checksum == input.checksum
        && existing.source_revision == input.source_revision
}

fn validate_chunk_input(
    chunk_index: i32,
    request_checksum: &str,
    entries: &[CacheEntryInput],
) -> Result<()> {
    if chunk_index < 0 {
        return Err(Error::validation(
            "cache ingest chunk index and request checksum are required",
        ));
    }
    validate_required_text(
        request_checksum,
        MAX_CACHE_TEXT_BYTES,
        "cache ingest request checksum",
    )?;
    if entries.len() > MAX_INGEST_CHUNK_RECORDS {
        return Err(Error::validation(
            "cache ingest chunk record limit exceeded",
        ));
    }

    let mut encoded_bytes = 0_usize;
    for entry in entries {
        validate_external_id(&entry.external_id)?;
        validate_optional_text(
            entry.source_checksum.as_deref(),
            MAX_CACHE_TEXT_BYTES,
            "cache entry source_checksum",
        )?;
        let value = serde_json::to_vec(&entry.value)
            .map_err(|_| Error::validation("cache entry value is not serializable"))?;
        if value.len() > MAX_CACHE_ENTRY_VALUE_BYTES {
            return Err(Error::validation("cache entry value byte limit exceeded"));
        }
        encoded_bytes = encoded_bytes
            .checked_add(entry.external_id.len())
            .and_then(|size| size.checked_add(value.len()))
            .and_then(|size| {
                size.checked_add(
                    entry
                        .source_checksum
                        .as_ref()
                        .map_or(0, |checksum| checksum.len()),
                )
            })
            .ok_or_else(|| Error::validation("cache ingest chunk is too large"))?;
        if encoded_bytes > MAX_INGEST_CHUNK_BYTES {
            return Err(Error::validation("cache ingest chunk byte limit exceeded"));
        }
    }
    Ok(())
}

fn ensure_namespace_writable(namespace: &CacheNamespace) -> Result<()> {
    if namespace.tombstoned_at.is_some() {
        return Err(Error::invalid_state("cache namespace is tombstoned"));
    }
    Ok(())
}

fn ensure_generation_quota(
    namespace: &CacheNamespace,
    record_count: i64,
    size_bytes: i64,
) -> Result<()> {
    if record_count > namespace.max_records_per_generation {
        return Err(Error::validation(
            "cache generation record quota would be exceeded",
        ));
    }
    if size_bytes > namespace.max_generation_bytes {
        return Err(Error::validation(
            "cache generation byte quota would be exceeded",
        ));
    }
    Ok(())
}

fn ensure_namespace_storage_quota(
    namespace: &CacheNamespace,
    aggregate_size_bytes: i64,
) -> Result<()> {
    if aggregate_size_bytes > namespace.max_retained_bytes {
        return Err(Error::validation(
            "cache namespace aggregate byte quota would be exceeded",
        ));
    }
    Ok(())
}

async fn lock_cache_admission(connection: &mut sqlx::PgConnection) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(CACHE_ADMISSION_ADVISORY_LOCK_KEY)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn ensure_namespace_admission(
    connection: &mut sqlx::PgConnection,
    owner_type: OwnerType,
    owner: &str,
    policy: &CacheAdmissionConfig,
) -> Result<()> {
    let (global_count, owner_count): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE owner_type = $1 AND owner = $2) \
         FROM cache_namespace WHERE tombstoned_at IS NULL",
    )
    .bind(owner_type)
    .bind(owner)
    .fetch_one(&mut *connection)
    .await?;
    if global_count >= policy.max_live_namespaces {
        return Err(Error::cache_quota_exceeded(
            "cache_global_namespace_limit_exceeded",
            "deployment live cache namespace limit exceeded",
        ));
    }
    if owner_count >= policy.max_live_namespaces_per_owner {
        return Err(Error::cache_quota_exceeded(
            "cache_owner_namespace_limit_exceeded",
            "cache owner live namespace limit exceeded",
        ));
    }
    Ok(())
}

async fn ensure_generation_admission(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    namespace: &CacheNamespace,
    expected_bytes: Option<i64>,
    policy: &CacheAdmissionConfig,
) -> Result<()> {
    let unpublished: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cache_generation g \
         JOIN cache_namespace n ON n.id = g.namespace \
         WHERE n.owner_type = $1 AND n.owner = $2 AND g.state IN ('staging', 'ready')",
    )
    .bind(namespace.owner_type)
    .bind(&namespace.owner)
    .fetch_one(&mut **tx)
    .await?;
    if unpublished >= policy.max_unpublished_generations_per_owner {
        return Err(Error::cache_quota_exceeded(
            "cache_owner_unpublished_generations_limit_exceeded",
            "cache owner unpublished generation limit exceeded",
        ));
    }
    ensure_physical_byte_admission_with_additional(
        tx,
        namespace,
        expected_bytes.unwrap_or(0),
        policy,
    )
    .await
}

async fn ensure_physical_byte_admission(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    namespace: &CacheNamespace,
    policy: &CacheAdmissionConfig,
) -> Result<()> {
    ensure_physical_byte_admission_with_additional(tx, namespace, 0, policy).await
}

async fn ensure_physical_byte_admission_with_additional(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    namespace: &CacheNamespace,
    additional_bytes: i64,
    policy: &CacheAdmissionConfig,
) -> Result<()> {
    let (global_bytes, owner_bytes): (i64, i64) = sqlx::query_as(
        "SELECT deployment.physical_bytes, COALESCE(owner.physical_bytes, 0) \
         FROM cache_deployment_physical_byte_usage deployment \
         LEFT JOIN cache_owner_physical_byte_usage owner \
           ON owner.owner_type = $1 AND owner.owner = $2 \
         WHERE deployment.id = 1",
    )
    .bind(namespace.owner_type)
    .bind(&namespace.owner)
    .fetch_one(&mut **tx)
    .await?;
    let projected_global = global_bytes.checked_add(additional_bytes).ok_or_else(|| {
        Error::cache_quota_exceeded(
            "cache_global_physical_bytes_limit_exceeded",
            "deployment physical cache byte limit exceeded",
        )
    })?;
    if projected_global > policy.max_physical_bytes {
        return Err(Error::cache_quota_exceeded(
            "cache_global_physical_bytes_limit_exceeded",
            "deployment physical cache byte limit exceeded",
        ));
    }
    let projected_owner = owner_bytes.checked_add(additional_bytes).ok_or_else(|| {
        Error::cache_quota_exceeded(
            "cache_owner_physical_bytes_limit_exceeded",
            "cache owner physical byte limit exceeded",
        )
    })?;
    if projected_owner > policy.max_physical_bytes_per_owner {
        return Err(Error::cache_quota_exceeded(
            "cache_owner_physical_bytes_limit_exceeded",
            "cache owner physical byte limit exceeded",
        ));
    }
    Ok(())
}

fn ensure_read_budget(bytes: i64, maximum: i64, operation: &str) -> Result<()> {
    if bytes > maximum {
        return Err(Error::validation(format!(
            "{operation} response byte limit exceeded"
        )));
    }
    Ok(())
}

fn validate_required_text(value: &str, maximum: usize, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > maximum {
        return Err(Error::validation(format!(
            "{field} must be nonempty and no more than {maximum} bytes"
        )));
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, maximum: usize, field: &str) -> Result<()> {
    if value.is_some_and(|value| value.len() > maximum) {
        return Err(Error::validation(format!(
            "{field} must be no more than {maximum} bytes"
        )));
    }
    Ok(())
}

fn push_cache_namespace_visibility_clause(
    query: &mut QueryBuilder<'_, Postgres>,
    visibility: &CacheNamespaceReadVisibility,
) {
    if visibility.grants.is_empty() {
        query.push("FALSE");
        return;
    }

    query.push("(");
    for (index, grant) in visibility.grants.iter().enumerate() {
        if index > 0 {
            query.push(" OR ");
        }
        push_cache_namespace_grant_clause(query, visibility.identity_id, grant);
    }
    query.push(")");
}

fn push_cache_namespace_grant_clause(
    query: &mut QueryBuilder<'_, Postgres>,
    identity_id: Id,
    grant: &CacheNamespaceGrantFilter,
) {
    query.push("(");
    let mut first = true;

    macro_rules! and_sep {
        () => {
            if first {
                first = false;
            } else {
                query.push(" AND ");
            }
        };
    }

    if let Some(owner) = grant.owner {
        and_sep!();
        match owner {
            OwnerConstraint::SelfOnly => {
                query.push("n.owner_identity = ");
                query.push_bind(identity_id);
            }
            OwnerConstraint::Any => {
                query.push("TRUE");
            }
            OwnerConstraint::None => {
                query.push("n.owner_identity IS NULL");
            }
        }
    }

    if let Some(owner_types) = &grant.owner_types {
        and_sep!();
        if owner_types.is_empty() {
            query.push("FALSE");
        } else {
            query.push("n.owner_type IN (");
            {
                let mut separated = query.separated(", ");
                for owner_type in owner_types {
                    separated.push_bind(*owner_type);
                }
            }
            query.push(")");
        }
    }

    if let Some(owner_refs) = &grant.owner_refs {
        and_sep!();
        if owner_refs.is_empty() {
            query.push("FALSE");
        } else {
            query.push("(CASE n.owner_type WHEN ");
            query.push_bind(OwnerType::Pack);
            query.push(" THEN n.owner_pack_ref WHEN ");
            query.push_bind(OwnerType::Action);
            query.push(" THEN n.owner_action_ref WHEN ");
            query.push_bind(OwnerType::Sensor);
            query.push(" THEN n.owner_sensor_ref ELSE NULL END) IN (");
            {
                let mut separated = query.separated(", ");
                for owner_ref in owner_refs {
                    separated.push_bind(owner_ref.clone());
                }
            }
            query.push(")");
        }
    }

    if let Some(namespace_refs) = &grant.namespace_refs {
        and_sep!();
        if namespace_refs.is_empty() {
            query.push("FALSE");
        } else {
            query.push("n.namespace IN (");
            {
                let mut separated = query.separated(", ");
                for namespace_ref in namespace_refs {
                    separated.push_bind(namespace_ref.clone());
                }
            }
            query.push(")");
        }
    }

    if first {
        query.push("TRUE");
    }
    query.push(")");
}

fn push_cache_namespace_freshness_clause(
    query: &mut QueryBuilder<'_, Postgres>,
    freshness: Option<CacheNamespaceFreshnessFilter>,
) {
    match freshness {
        None => {}
        Some(CacheNamespaceFreshnessFilter::Unpopulated) => {
            query.push(" AND n.active_generation IS NULL");
        }
        Some(CacheNamespaceFreshnessFilter::Fresh) => {
            query.push(
                " AND n.active_generation IS NOT NULL AND active.id IS NOT NULL \
                 AND (n.freshness_target_seconds <= 0 OR active.activated IS NULL \
                      OR active.activated >= NOW() \
                         - n.freshness_target_seconds * INTERVAL '1 second')",
            );
        }
        Some(CacheNamespaceFreshnessFilter::Stale) => {
            query.push(
                " AND n.active_generation IS NOT NULL AND active.id IS NOT NULL \
                 AND n.freshness_target_seconds > 0 AND active.activated IS NOT NULL \
                 AND active.activated < NOW() \
                     - n.freshness_target_seconds * INTERVAL '1 second'",
            );
        }
    }
}

fn namespace_page(mut items: Vec<CacheNamespace>, limit: i64) -> CacheNamespacePage {
    let has_more = items.len() > limit as usize;
    if has_more {
        items.pop();
    }
    let next_after_id = has_more.then(|| items.last().map(|item| item.id)).flatten();
    CacheNamespacePage {
        items,
        next_after_id,
    }
}

fn contains_like_pattern(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

fn bounded_limit(limit: i64, maximum: i64, operation: &str) -> Result<i64> {
    if !(1..=maximum).contains(&limit) {
        return Err(Error::validation(format!(
            "{operation} limit must be between 1 and {maximum}"
        )));
    }
    Ok(limit)
}

fn qualified_columns(alias: &str, columns: &str) -> String {
    columns
        .split(',')
        .map(|column| format!("{alias}.{}", column.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads the immutable owning namespace id for a generation without locking,
/// so callers can acquire the namespace lock before the generation lock.
async fn generation_namespace(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    generation_id: Id,
) -> Result<Option<Id>> {
    sqlx::query_scalar::<_, Id>("SELECT namespace FROM cache_generation WHERE id = $1")
        .bind(generation_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(Into::into)
}

async fn lock_namespace(
    connection: &mut sqlx::PgConnection,
    namespace_id: Id,
) -> Result<Option<CacheNamespace>> {
    let query = format!(
        "SELECT {CACHE_NAMESPACE_SELECT_COLUMNS} FROM cache_namespace WHERE id = $1 FOR UPDATE"
    );
    sqlx::query_as::<_, CacheNamespace>(&query)
        .bind(namespace_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(Into::into)
}

async fn lock_generation(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    generation_id: Id,
) -> Result<Option<CacheGeneration>> {
    let query = format!(
        "SELECT {CACHE_GENERATION_SELECT_COLUMNS} FROM cache_generation WHERE id = $1 FOR UPDATE"
    );
    sqlx::query_as::<_, CacheGeneration>(&query)
        .bind(generation_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_scope_requires_matching_canonical_owner() {
        assert_eq!(
            CacheOwnerScope::system().canonical_owner().unwrap(),
            "system"
        );
        assert_eq!(
            CacheOwnerScope::identity(42).canonical_owner().unwrap(),
            "42"
        );

        let mut invalid = CacheOwnerScope::pack(7, Some("example".to_string()));
        invalid.owner_action = Some(8);
        assert!(invalid.canonical_owner().is_err());
    }

    #[test]
    fn validates_namespace_and_external_identifier_bounds() {
        assert!(validate_namespace_name("salesforce.users").is_ok());
        assert!(validate_namespace_name("Salesforce").is_err());
        assert!(validate_namespace_name("bad/name").is_err());
        assert!(validate_external_id("Case-Sensitive").is_ok());
        assert!(validate_external_id("").is_err());
    }

    #[test]
    fn bounded_operations_reject_unbounded_requests() {
        assert!(bounded_limit(0, MAX_SCAN_PAGE_SIZE, "scan").is_err());
        assert!(bounded_limit(MAX_SCAN_PAGE_SIZE + 1, MAX_SCAN_PAGE_SIZE, "scan").is_err());
        assert_eq!(
            bounded_limit(MAX_SCAN_PAGE_SIZE, MAX_SCAN_PAGE_SIZE, "scan").unwrap(),
            MAX_SCAN_PAGE_SIZE
        );
    }

    #[test]
    fn namespace_policy_requires_active_and_prior_generation_capacity() {
        let policy = CacheNamespacePolicy {
            max_retained_generations: 1,
            ..CacheNamespacePolicy::default()
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn namespace_contains_filter_escapes_like_wildcards() {
        assert_eq!(
            contains_like_pattern(r"team_100%\cache"),
            r"%team\_100\%\\cache%"
        );
    }
}
