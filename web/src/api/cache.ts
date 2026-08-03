/**
 * Hand-authored compatibility facade for the owner-scoped Data Caches API.
 *
 * This file follows the same pattern as `./queues.ts`, `./retention.ts`, and
 * `./workers.ts`: it lives alongside the generated client (`core/`, `models/`,
 * `services/`, `index.ts`) but is **not* * overwritten by `npm run generate:api`. The backend OpenAPI spec now includes
 * the cache routes and the standard generator was verified against them, but
 * that command replaces the full `src/api` tree and would erase deliberate
 * extension files (`queues.ts`, `retention.ts`, `workers.ts`, and this facade)
 * while also introducing broad unrelated generated drift. Keep this narrow
 * facade until the repository adopts a generator workflow that preserves
 * extensions.
 *
 * STATUS: `crates/api/src/routes/cache.rs` / `crates/api/src/dto/cache.rs`
 * exist in the working tree and are captured by exported OpenAPI. The shapes
 * below are kept field-for-field with those Rust files and provide stable
 * owner/route helpers for the UI.
 *
 * Key contract notes (see crates/api/src/routes/cache.rs for the source of
 * truth):
 *   - Every route requires `owner_type` (never optional) plus an optional
 *     `owner_ref`, delivered as query params for GET/DELETE and as body
 *     fields for POST/PUT. `owner_ref` is ignored for `system`/`identity`.
 *   - Only `{namespace}` (and `{generation_id}` / `{chunk_index}` where
 *     relevant) are path segments — owner scope is never part of the path.
 *   - Namespace and generation metadata use opaque keyset cursors. Namespace
 *     listing also supports server-side namespace-substring and freshness
 *     filters; callers must preserve the returned cursor's page shape.
 *   - Chunk upload is `PUT .../chunks/{chunk_index}` with a JSON body
 *     (`{owner_type, owner_ref, entries: [...]}`), not raw NDJSON text. The
 *     server derives the idempotency checksum from the raw request bytes
 *     itself; the client does not compute or send one.
 *   - Cache-specific error conditions (`cache_not_populated`,
 *     `snapshot_expired`, `cache_precondition_failed`, `cache_conflict`,
 *     `cache_quota_exceeded`, `cache_stale`, `namespace_deleted`,
 *     `cache_cursor_invalid`) are carried in the response body's `code`
 *     field (not `error`, which holds the human-readable message), almost
 *     always alongside HTTP 409.
 *   - `abandon` does not accept a caller-supplied reason; the server records
 *     a fixed "refresh abandoned" reason.
 *
 * Regenerating: preserve all top-level extension files before replacing the
 * generated `core/`, `models/`, `services/`, and `index.ts` paths, then
 * reconcile this facade only when the generated surface can be adopted
 * without unrelated churn.
 */
import type { CancelablePromise } from "./core/CancelablePromise";
import { OpenAPI } from "./core/OpenAPI";
import { request as __request } from "./core/request";
import { OwnerType } from "./models/OwnerType";

export { OwnerType };

export type JsonValue =
  string | number | boolean | null | { [key: string]: JsonValue } | JsonValue[];

export interface ApiResponse<T> {
  data: T;
  message?: string | null;
}

/**
 * A single canonical owner ref segment used in the web route
 * (`/caches/:ownerType/:ownerRef/:namespace`). Owner types with no
 * meaningful denormalized ref use a reserved placeholder instead of a real
 * ref string, purely for that client-side route — the API itself never
 * expects a placeholder value, it expects `owner_ref` to be omitted:
 *   - `system` has no owner ref at all -> `SYSTEM_OWNER_REF_PLACEHOLDER`.
 *   - `identity` only supports the caller's own identity (see
 *     KEY_CACHE.md) -> `SELF_OWNER_REF_PLACEHOLDER`.
 */
export const SYSTEM_OWNER_REF_PLACEHOLDER = "_";
export const SELF_OWNER_REF_PLACEHOLDER = "self";

export function ownerRefPlaceholder(ownerType: OwnerType): string {
  if (ownerType === OwnerType.SYSTEM) return SYSTEM_OWNER_REF_PLACEHOLDER;
  if (ownerType === OwnerType.IDENTITY) return SELF_OWNER_REF_PLACEHOLDER;
  return "";
}

/** Lifecycle state of an immutable cache generation (`CacheGenerationState`). */
export enum CacheGenerationState {
  STAGING = "staging",
  READY = "ready",
  ACTIVE = "active",
  RETIRED = "retired",
  FAILED = "failed",
}

export enum CacheNamespaceFreshness {
  FRESH = "fresh",
  STALE = "stale",
  UNPOPULATED = "unpopulated",
}

/** Machine-readable cache error codes carried in `ApiError.body.code`. */
export enum CacheErrorCode {
  NOT_POPULATED = "cache_not_populated",
  SNAPSHOT_EXPIRED = "snapshot_expired",
  NAMESPACE_DELETED = "namespace_deleted",
  QUOTA_EXCEEDED = "cache_quota_exceeded",
  CONFLICT = "cache_conflict",
  PRECONDITION_FAILED = "cache_precondition_failed",
  STALE = "cache_stale",
  CURSOR_INVALID = "cache_cursor_invalid",
  DUPLICATE_EXTERNAL_ID = "cache_duplicate_external_id",
}

/** Owner selector used by every cache service call. */
export interface CacheOwnerParams {
  ownerType: OwnerType;
  /** Denormalized owner ref (pack/action/sensor ref). Ignored for system/identity. */
  ownerRef?: string | null;
}

/**
 * Owner-scoped, generation-based cache namespace metadata. Mirrors
 * `attune_api::dto::cache::CacheNamespaceResponse` field-for-field. Never
 * includes entry values.
 */
export interface CacheNamespaceResponse {
  id: number;
  owner_type: OwnerType;
  /** Canonical owner key rendered by the database (e.g. `system`, or an id). */
  owner: string;
  /** Denormalized owner reference for display, when known. */
  owner_ref: string | null;
  namespace: string;
  /** True when policy and lifecycle are declared by an installed pack. */
  managed: boolean;
  /** Declarative cache definition ref within the managing pack, when managed. */
  definition_ref: string | null;
  /** Pack ref that owns the declarative definition, when managed. */
  managing_pack_ref: string | null;
  active_generation: number | null;
  freshness_target_seconds: number;
  max_records_per_generation: number;
  max_generation_bytes: number;
  max_retained_bytes: number;
  max_retained_generations: number;
  max_staging_generations: number;
  /** Whether the namespace is tombstoned and pending bounded cleanup. */
  tombstoned: boolean;
  created: string;
  updated: string;
  /** True when there is no active generation (uninitialized dataset). */
  cache_not_populated: boolean;
  /** True when the active generation's age exceeds the freshness target. */
  stale: boolean;
  record_count: number | null;
  size_bytes: number | null;
  source_revision: string | null;
  last_refreshed_at: string | null;
}

/** Mirrors `CacheNamespaceDeletionResponse`. */
export interface CacheNamespaceDeletionResponse {
  id: number;
  namespace: string;
  tombstoned: boolean;
  /** Cleanup is asynchronous; entries are reclaimed in bounded batches. */
  cleanup_pending: boolean;
  status: string;
}

/** Mirrors `CacheGenerationResponse` field-for-field (also the refresh-lifecycle response shape). */
export interface CacheGenerationResponse {
  generation_id: number;
  namespace_id: number;
  status: CacheGenerationState;
  client_refresh_id: string;
  expected_active_generation_id: number | null;
  expected_chunk_count: number;
  expected_record_count: number | null;
  expected_size_bytes: number | null;
  record_count: number;
  size_bytes: number;
  checksum_algorithm: string | null;
  checksum: string | null;
  source_revision: string | null;
  created_by: number | null;
  created: string;
  sealed: string | null;
  activated: string | null;
  retired: string | null;
  readable_until: string | null;
  failed: string | null;
  failure_reason: string | null;
}

/** Mirrors `CacheEntryResponse`. Deliberately has no `id`/`generation` fields. */
export interface CacheEntryResponse {
  external_id: string;
  value: JsonValue;
  source_updated_at: string | null;
  source_checksum: string | null;
  size_bytes: number;
}

export interface CachePointLookupResponse {
  generation_id: number;
  item: CacheEntryResponse | null;
  stale: boolean;
}

export interface CacheMultiLookupResponse {
  generation_id: number;
  items: CacheEntryResponse[];
  /** Requested external IDs that had no active entry, in request order. */
  missing_external_ids: string[];
  stale: boolean;
}

export interface CacheScanPageResponse {
  /** The pinned generation this page (and every subsequent page) reads from. */
  generation_id: number;
  items: CacheEntryResponse[];
  next_cursor: string | null;
  cursor_expires_at: string | null;
  /** Total record count of the pinned generation, when known. */
  record_count: number | null;
  stale: boolean;
}

export interface CacheNamespaceListResponse {
  namespaces: CacheNamespaceResponse[];
  next_cursor: string | null;
}

export interface CacheGenerationListResponse {
  generations: CacheGenerationResponse[];
  next_cursor: string | null;
}

export interface CreateCacheNamespaceRequest {
  owner_type: OwnerType;
  owner_ref?: string | null;
  namespace: string;
  freshness_target_seconds?: number;
  max_records_per_generation?: number;
  max_generation_bytes?: number;
  max_retained_bytes?: number;
  max_retained_generations?: number;
  max_staging_generations?: number;
}

export interface UpdateCacheNamespaceRequest {
  owner_type: OwnerType;
  owner_ref?: string | null;
  freshness_target_seconds?: number;
  max_records_per_generation?: number;
  max_generation_bytes?: number;
  max_retained_bytes?: number;
  max_retained_generations?: number;
  max_staging_generations?: number;
}

export interface CacheEntryUpload {
  external_id: string;
  value: JsonValue;
  source_updated_at?: string | null;
  source_checksum?: string | null;
}

export interface CreateCacheGenerationRequest {
  owner_type: OwnerType;
  owner_ref?: string | null;
  /** Client-chosen idempotency key; replaying it returns the same generation. */
  client_refresh_id: string;
  expected_active_generation_id: number | null;
  expected_chunk_count: number;
  expected_record_count?: number | null;
  expected_size_bytes?: number | null;
  source_revision?: string | null;
}

export interface SealCacheGenerationRequest {
  owner_type: OwnerType;
  owner_ref?: string | null;
  expected_chunk_count: number;
  expected_record_count?: number | null;
  expected_size_bytes?: number | null;
}

export interface PromoteCacheGenerationRequest {
  owner_type: OwnerType;
  owner_ref?: string | null;
  /** `null` explicitly means "this is the first publication". */
  expected_active_generation_id: number | null;
}

function ownerQuery(owner: CacheOwnerParams) {
  return {
    owner_type: owner.ownerType,
    owner_ref: owner.ownerRef || undefined,
  };
}

function ownerBody(owner: CacheOwnerParams) {
  return {
    owner_type: owner.ownerType,
    owner_ref: owner.ownerRef || undefined,
  };
}

const NAMESPACE_BASE = "/api/v1/cache/namespaces";
const NAMESPACE_DETAIL = `${NAMESPACE_BASE}/{namespace}`;

const CACHE_ERRORS = {
  400: "Validation error",
  403: "Insufficient permissions",
  404: "Not found",
  409: "Conflict",
};

export class CachesService {
  // ── Namespaces ────────────────────────────────────────────────────────

  public static listNamespaces({
    owner,
    namespace,
    freshness,
    limit,
    cursor,
  }: {
    owner: CacheOwnerParams;
    namespace?: string;
    freshness?: CacheNamespaceFreshness;
    limit?: number;
    cursor?: string;
  }): CancelablePromise<ApiResponse<CacheNamespaceListResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: NAMESPACE_BASE,
      query: {
        ...ownerQuery(owner),
        namespace,
        freshness,
        limit,
        cursor,
      },
      errors: CACHE_ERRORS,
    });
  }

  public static getNamespace({
    owner,
    namespace,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
  }): CancelablePromise<ApiResponse<CacheNamespaceResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: NAMESPACE_DETAIL,
      path: { namespace },
      query: ownerQuery(owner),
      errors: { ...CACHE_ERRORS, 404: "Cache namespace not found" },
    });
  }

  public static createNamespace({
    requestBody,
  }: {
    requestBody: CreateCacheNamespaceRequest;
  }): CancelablePromise<ApiResponse<CacheNamespaceResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: NAMESPACE_BASE,
      body: requestBody,
      mediaType: "application/json",
      errors: {
        ...CACHE_ERRORS,
        409: "Cache namespace already exists in this owner scope",
      },
    });
  }

  public static updateNamespacePolicy({
    namespace,
    requestBody,
  }: {
    namespace: string;
    requestBody: UpdateCacheNamespaceRequest;
  }): CancelablePromise<ApiResponse<CacheNamespaceResponse>> {
    return __request(OpenAPI, {
      method: "PUT",
      url: NAMESPACE_DETAIL,
      path: { namespace },
      body: requestBody,
      mediaType: "application/json",
      errors: { ...CACHE_ERRORS, 404: "Cache namespace not found" },
    });
  }

  /** Tombstones the namespace immediately; data is reclaimed asynchronously. */
  public static deleteNamespace({
    owner,
    namespace,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
  }): CancelablePromise<ApiResponse<CacheNamespaceDeletionResponse>> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: NAMESPACE_DETAIL,
      path: { namespace },
      query: ownerQuery(owner),
      errors: { ...CACHE_ERRORS, 404: "Cache namespace not found" },
    });
  }

  // ── Generations ───────────────────────────────────────────────────────

  public static listGenerations({
    owner,
    namespace,
    limit,
    cursor,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
    limit?: number;
    cursor?: string;
  }): CancelablePromise<ApiResponse<CacheGenerationListResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: `${NAMESPACE_DETAIL}/generations`,
      path: { namespace },
      query: {
        ...ownerQuery(owner),
        limit,
        cursor,
      },
      errors: { ...CACHE_ERRORS, 404: "Cache namespace not found" },
    });
  }

  public static getGeneration({
    owner,
    namespace,
    generationId,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
    generationId: number;
  }): CancelablePromise<ApiResponse<CacheGenerationResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: `${NAMESPACE_DETAIL}/generations/{generation_id}`,
      path: { namespace, generation_id: generationId },
      query: ownerQuery(owner),
      errors: { ...CACHE_ERRORS, 404: "Cache generation not found" },
    });
  }

  // ── Entries (deliberate, bounded reads only) ─────────────────────────

  public static getEntry({
    owner,
    namespace,
    externalId,
    generationId,
    requireFresh,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
    externalId: string;
    generationId?: number;
    requireFresh?: boolean;
  }): CancelablePromise<ApiResponse<CachePointLookupResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: `${NAMESPACE_DETAIL}/entries/lookup`,
      path: { namespace },
      body: {
        ...ownerBody(owner),
        external_id: externalId,
        generation_id: generationId,
        require_fresh: requireFresh || undefined,
      },
      mediaType: "application/json",
      errors: CACHE_ERRORS,
    });
  }

  /** Bounded multi-ID lookup. Body-delivered so IDs never land in request logs/URLs. */
  public static getEntries({
    owner,
    namespace,
    externalIds,
    generationId,
    requireFresh,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
    externalIds: string[];
    generationId?: number;
    requireFresh?: boolean;
  }): CancelablePromise<ApiResponse<CacheMultiLookupResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: `${NAMESPACE_DETAIL}/entries/lookup-many`,
      path: { namespace },
      body: {
        ...ownerBody(owner),
        external_ids: externalIds,
        generation_id: generationId,
        require_fresh: requireFresh || undefined,
      },
      mediaType: "application/json",
      errors: CACHE_ERRORS,
    });
  }

  /**
   * Generation-pinned keyset scan. Omit `generationId`/`cursor` for the first
   * page; every subsequent page must pass back the `generation_id` and
   * `next_cursor` the server returned.
   */
  public static scanEntries({
    owner,
    namespace,
    generationId,
    cursor,
    limit,
    requireFresh,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
    generationId?: number;
    cursor?: string;
    limit?: number;
    requireFresh?: boolean;
  }): CancelablePromise<ApiResponse<CacheScanPageResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: `${NAMESPACE_DETAIL}/entries`,
      path: { namespace },
      query: {
        ...ownerQuery(owner),
        generation: generationId,
        cursor: cursor || undefined,
        limit,
        require_fresh: requireFresh || undefined,
      },
      errors: CACHE_ERRORS,
    });
  }

  // ── Refresh lifecycle ─────────────────────────────────────────────────

  public static beginRefresh({
    namespace,
    requestBody,
  }: {
    namespace: string;
    requestBody: CreateCacheGenerationRequest;
  }): CancelablePromise<ApiResponse<CacheGenerationResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: `${NAMESPACE_DETAIL}/generations`,
      path: { namespace },
      body: requestBody,
      mediaType: "application/json",
      errors: CACHE_ERRORS,
    });
  }

  /** Uploads one atomic, numbered chunk. Retrying the same index+content is a no-op. */
  public static uploadRefreshChunk({
    owner,
    namespace,
    generationId,
    chunkIndex,
    entries,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
    generationId: number;
    chunkIndex: number;
    entries: CacheEntryUpload[];
  }): CancelablePromise<ApiResponse<CacheGenerationResponse>> {
    return __request(OpenAPI, {
      method: "PUT",
      url: `${NAMESPACE_DETAIL}/generations/{generation_id}/chunks/{chunk_index}`,
      path: {
        namespace,
        generation_id: generationId,
        chunk_index: chunkIndex,
      },
      body: { ...ownerBody(owner), entries },
      mediaType: "application/json",
      errors: {
        ...CACHE_ERRORS,
        409: "Chunk index already accepted with different content",
      },
    });
  }

  public static sealGeneration({
    namespace,
    generationId,
    requestBody,
  }: {
    namespace: string;
    generationId: number;
    requestBody: SealCacheGenerationRequest;
  }): CancelablePromise<ApiResponse<CacheGenerationResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: `${NAMESPACE_DETAIL}/generations/{generation_id}/seal`,
      path: { namespace, generation_id: generationId },
      body: requestBody,
      mediaType: "application/json",
      errors: CACHE_ERRORS,
    });
  }

  /** Optimistic promotion; a 409 `cache_precondition_failed` means the active generation changed. */
  public static promoteGeneration({
    namespace,
    generationId,
    requestBody,
  }: {
    namespace: string;
    generationId: number;
    requestBody: PromoteCacheGenerationRequest;
  }): CancelablePromise<ApiResponse<CacheGenerationResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: `${NAMESPACE_DETAIL}/generations/{generation_id}/promote`,
      path: { namespace, generation_id: generationId },
      body: requestBody,
      mediaType: "application/json",
      errors: CACHE_ERRORS,
    });
  }

  /** Fails a staging or ready generation. It never becomes visible. The server, not the caller, records the reason. */
  public static abandonGeneration({
    owner,
    namespace,
    generationId,
  }: {
    owner: CacheOwnerParams;
    namespace: string;
    generationId: number;
  }): CancelablePromise<ApiResponse<CacheGenerationResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: `${NAMESPACE_DETAIL}/generations/{generation_id}/abandon`,
      path: { namespace, generation_id: generationId },
      body: ownerBody(owner),
      mediaType: "application/json",
      errors: CACHE_ERRORS,
    });
  }
}
