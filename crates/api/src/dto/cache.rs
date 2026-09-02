//! Owner-scoped cache data transfer objects.
//!
//! These types define the OpenAPI-documented HTTP contract consumed by
//! generated SDKs, the CLI, and the web UI. Cache values are plain business
//! data returned over the API; they are never secret-injected. Generation and
//! cursor fields keep paged clients pinned to one immutable snapshot so a
//! promotion cannot mix pages from different generations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};

use attune_common::models::{cache::CacheEntry, CacheGenerationState, Id, OwnerType};

use crate::{auth::middleware::AuthErrorResponse, middleware::error::ErrorResponse};

/// Owner selector accepted in cache request bodies.
///
/// `owner_ref` is the pack/action/sensor reference; it is omitted for the
/// `system` scope and resolved to the authenticated identity for `identity`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CacheOwnerBody {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
}

/// Owner selector accepted as query parameters for GET/DELETE routes.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct CacheOwnerQuery {
    /// Owner type: `system`, `identity`, `pack`, `action`, or `sensor`.
    pub owner_type: OwnerType,
    /// Owner reference (pack/action/sensor ref). Omitted for system scope.
    #[serde(default)]
    pub owner_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheNamespaceFreshness {
    Fresh,
    Stale,
    Unpopulated,
}

/// Namespace metadata filters and keyset pagination. Omit `owner_type` to list
/// namespaces across every owner scope visible to the caller.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct CacheNamespaceListQuery {
    #[serde(default)]
    pub owner_type: Option<OwnerType>,
    #[serde(default)]
    pub owner_ref: Option<String>,
    /// Case-insensitive namespace substring.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by active-generation freshness state.
    #[serde(default)]
    pub freshness: Option<CacheNamespaceFreshness>,
    /// Requested page size (bounded server-side).
    #[serde(default)]
    #[param(minimum = 1, maximum = 500, example = 100)]
    pub limit: Option<i64>,
    /// Opaque keyset cursor from a prior page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Namespace-scoped generation metadata keyset pagination.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct CacheGenerationListQuery {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    /// Requested page size (bounded server-side).
    #[serde(default)]
    #[param(minimum = 1, maximum = 500, example = 100)]
    pub limit: Option<i64>,
    /// Opaque keyset cursor from a prior page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Namespace-level publication policy overrides. Unspecified fields keep their
/// existing (or default) values.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct CacheNamespacePolicyBody {
    pub freshness_target_seconds: Option<i64>,
    pub max_records_per_generation: Option<i64>,
    pub max_generation_bytes: Option<i64>,
    pub max_retained_bytes: Option<i64>,
    /// Number of published generations retained. At least two are required so
    /// readers can complete traversal of the prior snapshot after promotion.
    #[schema(minimum = 2, example = 2)]
    pub max_retained_generations: Option<i32>,
    pub max_staging_generations: Option<i32>,
}

/// Create a new owner-scoped cache namespace.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateCacheNamespaceRequest {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    /// Normalized lowercase namespace, e.g. `salesforce.users`.
    #[schema(
        pattern = "^[a-z0-9][a-z0-9._-]{0,127}$",
        max_length = 128,
        example = "salesforce.users"
    )]
    pub namespace: String,
    #[serde(flatten)]
    pub policy: CacheNamespacePolicyBody,
}

/// Update a namespace's publication policy. Owner scope and namespace are
/// immutable; changing either is a new namespace.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct UpdateCacheNamespaceRequest {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    #[serde(flatten)]
    pub policy: CacheNamespacePolicyBody,
}

/// Namespace metadata and freshness/health summary. Never includes entries.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CacheNamespaceResponse {
    pub id: Id,
    pub owner_type: OwnerType,
    /// Canonical owner key (`system` or a numeric owner id as text).
    pub owner: String,
    /// Owner reference for display, when known.
    #[schema(required = true, nullable = true)]
    pub owner_ref: Option<String>,
    /// Whether this namespace is declaratively managed by a pack definition.
    pub managed: bool,
    /// Stable declarative component ref for a pack-managed namespace.
    #[schema(required = true, nullable = true)]
    pub definition_ref: Option<String>,
    /// Durable ref of the pack that manages this namespace.
    #[schema(required = true, nullable = true)]
    pub managing_pack_ref: Option<String>,
    pub namespace: String,
    #[schema(required = true, nullable = true)]
    pub active_generation: Option<Id>,
    pub freshness_target_seconds: i64,
    pub max_records_per_generation: i64,
    pub max_generation_bytes: i64,
    pub max_retained_bytes: i64,
    pub max_retained_generations: i32,
    pub max_staging_generations: i32,
    /// Whether the namespace is tombstoned and pending bounded cleanup.
    pub tombstoned: bool,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    /// True when there is no active generation (uninitialized dataset).
    pub cache_not_populated: bool,
    /// True when the active generation's age exceeds the freshness target.
    pub stale: bool,
    /// Active generation record count, when populated.
    #[schema(required = true, nullable = true)]
    pub record_count: Option<i64>,
    /// Active generation size in bytes, when populated.
    #[schema(required = true, nullable = true)]
    pub size_bytes: Option<i64>,
    /// Active generation source revision, when populated.
    #[schema(required = true, nullable = true)]
    pub source_revision: Option<String>,
    /// When the active generation was published.
    #[schema(required = true, nullable = true)]
    pub last_refreshed_at: Option<DateTime<Utc>>,
}

/// Wrapper for a namespace list scoped to one owner.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CacheNamespaceListResponse {
    pub namespaces: Vec<CacheNamespaceResponse>,
    #[schema(required = true, nullable = true)]
    pub next_cursor: Option<String>,
}

/// Tombstone/queued-cleanup status returned by namespace deletion.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CacheNamespaceDeletionResponse {
    pub id: Id,
    pub namespace: String,
    pub tombstoned: bool,
    /// Cleanup is asynchronous; entries are reclaimed in bounded batches.
    pub cleanup_pending: bool,
    pub status: String,
}

/// Immutable generation metadata. Also serves as the refresh-lifecycle
/// operation response for create/upload/seal/promote/abandon.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CacheGenerationResponse {
    /// Generation identifier (accepted by the client as `generation`/`id`).
    pub generation_id: Id,
    pub namespace_id: Id,
    /// Lifecycle state: `staging`, `ready`, `active`, `retired`, or `failed`.
    pub status: CacheGenerationState,
    pub client_refresh_id: String,
    /// Optimistic active-generation value captured when this refresh began.
    /// `null` means the namespace was expected to be unpopulated.
    #[schema(required = true, nullable = true)]
    pub expected_active_generation_id: Option<Id>,
    pub expected_chunk_count: i32,
    #[schema(required = true, nullable = true)]
    pub expected_record_count: Option<i64>,
    #[schema(required = true, nullable = true)]
    pub expected_size_bytes: Option<i64>,
    pub record_count: i64,
    pub size_bytes: i64,
    #[schema(required = true, nullable = true)]
    pub checksum_algorithm: Option<String>,
    #[schema(required = true, nullable = true)]
    pub checksum: Option<String>,
    #[schema(required = true, nullable = true)]
    pub source_revision: Option<String>,
    #[schema(required = true, nullable = true)]
    pub created_by: Option<Id>,
    pub created: DateTime<Utc>,
    #[schema(required = true, nullable = true)]
    pub sealed: Option<DateTime<Utc>>,
    #[schema(required = true, nullable = true)]
    pub activated: Option<DateTime<Utc>>,
    #[schema(required = true, nullable = true)]
    pub retired: Option<DateTime<Utc>>,
    #[schema(required = true, nullable = true)]
    pub readable_until: Option<DateTime<Utc>>,
    #[schema(required = true, nullable = true)]
    pub failed: Option<DateTime<Utc>>,
    #[schema(required = true, nullable = true)]
    pub failure_reason: Option<String>,
}

/// Wrapper for a generation list.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CacheGenerationListResponse {
    pub generations: Vec<CacheGenerationResponse>,
    #[schema(required = true, nullable = true)]
    pub next_cursor: Option<String>,
}

/// A single cache record. Extra descriptive fields beyond `external_id`/`value`
/// are ignored by minimal clients.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CacheEntryResponse {
    pub external_id: String,
    #[schema(value_type = Value)]
    pub value: JsonValue,
    #[schema(required = true, nullable = true)]
    pub source_updated_at: Option<DateTime<Utc>>,
    #[schema(required = true, nullable = true)]
    pub source_checksum: Option<String>,
    pub size_bytes: i64,
}

impl From<CacheEntry> for CacheEntryResponse {
    fn from(entry: CacheEntry) -> Self {
        Self {
            external_id: entry.external_id,
            value: entry.value,
            source_updated_at: entry.source_updated_at,
            source_checksum: entry.source_checksum,
            size_bytes: entry.size_bytes,
        }
    }
}

/// Point lookup request. Identifiers are placed in the body to avoid access-log
/// leakage.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CachePointLookupRequest {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    pub external_id: String,
    /// Optional explicit generation pin. Active and still-readable retired
    /// generations may be read; an expired pin returns `snapshot_expired`.
    #[serde(default)]
    pub generation_id: Option<Id>,
    #[serde(default)]
    pub require_fresh: bool,
}

/// Point lookup response. `item = None` is an authorized miss.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CachePointLookupResponse {
    pub generation_id: Id,
    #[schema(required = true, nullable = true)]
    pub item: Option<CacheEntryResponse>,
    pub stale: bool,
}

/// Bounded multi-ID lookup request.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CacheMultiLookupRequest {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    pub external_ids: Vec<String>,
    #[serde(default)]
    pub generation_id: Option<Id>,
    #[serde(default)]
    pub require_fresh: bool,
}

/// Bounded multi-ID lookup response. Missing IDs are reported explicitly.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CacheMultiLookupResponse {
    pub generation_id: Id,
    pub items: Vec<CacheEntryResponse>,
    pub missing_external_ids: Vec<String>,
    pub stale: bool,
}

/// Generation-pinned cursor scan query parameters.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct CacheScanQuery {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    /// Requested page size (bounded server-side).
    #[serde(default)]
    #[param(minimum = 1, maximum = 1000, example = 100)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub require_fresh: bool,
    /// Pinned generation. Required together with `cursor` on later pages.
    #[serde(default)]
    pub generation: Option<Id>,
    /// Opaque, integrity-protected cursor from a prior page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// One generation-pinned scan page.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CacheScanPageResponse {
    pub generation_id: Id,
    pub items: Vec<CacheEntryResponse>,
    #[schema(required = true, nullable = true)]
    pub next_cursor: Option<String>,
    #[schema(required = true, nullable = true)]
    pub cursor_expires_at: Option<DateTime<Utc>>,
    #[schema(required = true, nullable = true)]
    pub record_count: Option<i64>,
    pub stale: bool,
}

/// Create (begin) a staging generation.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateCacheGenerationRequest {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    pub client_refresh_id: String,
    /// Required optimistic guard observed by the caller. Explicit `null`
    /// means this is expected to be the namespace's first publication.
    #[schema(required = true, nullable = true)]
    pub expected_active_generation_id: Option<Id>,
    /// Declared chunk count for contiguity validation at seal time.
    pub expected_chunk_count: i64,
    #[serde(default)]
    pub expected_record_count: Option<i64>,
    #[serde(default)]
    pub expected_size_bytes: Option<i64>,
    #[serde(default)]
    pub source_revision: Option<String>,
}

/// A record inside an upload chunk.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CacheEntryUpload {
    pub external_id: String,
    #[schema(value_type = Value)]
    pub value: JsonValue,
    #[serde(default)]
    pub source_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub source_checksum: Option<String>,
}

/// Upload one numbered ingest chunk.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct UploadCacheChunkRequest {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    pub entries: Vec<CacheEntryUpload>,
}

/// Seal a staging generation into `ready`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SealCacheGenerationRequest {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    pub expected_chunk_count: i64,
    #[serde(default)]
    pub expected_record_count: Option<i64>,
    #[serde(default)]
    pub expected_size_bytes: Option<i64>,
}

/// Atomically promote a ready generation.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PromoteCacheGenerationRequest {
    pub owner_type: OwnerType,
    #[serde(default)]
    pub owner_ref: Option<String>,
    /// Required optimistic guard; explicit `null` means first publication.
    #[schema(required = true, nullable = true)]
    pub expected_active_generation_id: Option<Id>,
}

macro_rules! cache_api_response {
    ($name:ident, $data:ty) => {
        #[derive(Debug, Clone, Serialize, ToSchema)]
        pub struct $name {
            pub data: $data,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub message: Option<String>,
        }
    };
}

cache_api_response!(CacheNamespaceApiResponse, CacheNamespaceResponse);
cache_api_response!(CacheNamespaceListApiResponse, CacheNamespaceListResponse);
cache_api_response!(
    CacheNamespaceDeletionApiResponse,
    CacheNamespaceDeletionResponse
);
cache_api_response!(CacheGenerationApiResponse, CacheGenerationResponse);
cache_api_response!(CacheGenerationListApiResponse, CacheGenerationListResponse);
cache_api_response!(CachePointLookupApiResponse, CachePointLookupResponse);
cache_api_response!(CacheMultiLookupApiResponse, CacheMultiLookupResponse);
cache_api_response!(CacheScanPageApiResponse, CacheScanPageResponse);

/// A cache request can be rejected either by the authentication extractor or
/// by cache RBAC after authentication succeeds.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub enum CacheForbiddenResponse {
    Authentication(AuthErrorResponse),
    Authorization(ErrorResponse),
}
