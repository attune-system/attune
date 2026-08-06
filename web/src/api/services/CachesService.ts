/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheGenerationApiResponse } from "../models/CacheGenerationApiResponse";
import type { CacheGenerationListApiResponse } from "../models/CacheGenerationListApiResponse";
import type { CacheMultiLookupApiResponse } from "../models/CacheMultiLookupApiResponse";
import type { CacheMultiLookupRequest } from "../models/CacheMultiLookupRequest";
import type { CacheNamespaceApiResponse } from "../models/CacheNamespaceApiResponse";
import type { CacheNamespaceDeletionApiResponse } from "../models/CacheNamespaceDeletionApiResponse";
import type { CacheNamespaceFreshness } from "../models/CacheNamespaceFreshness";
import type { CacheNamespaceListApiResponse } from "../models/CacheNamespaceListApiResponse";
import type { CacheOwnerBody } from "../models/CacheOwnerBody";
import type { CachePointLookupApiResponse } from "../models/CachePointLookupApiResponse";
import type { CachePointLookupRequest } from "../models/CachePointLookupRequest";
import type { CacheScanPageApiResponse } from "../models/CacheScanPageApiResponse";
import type { CreateCacheGenerationRequest } from "../models/CreateCacheGenerationRequest";
import type { CreateCacheNamespaceRequest } from "../models/CreateCacheNamespaceRequest";
import type { i64 } from "../models/i64";
import type { OwnerType } from "../models/OwnerType";
import type { PromoteCacheGenerationRequest } from "../models/PromoteCacheGenerationRequest";
import type { SealCacheGenerationRequest } from "../models/SealCacheGenerationRequest";
import type { UpdateCacheNamespaceRequest } from "../models/UpdateCacheNamespaceRequest";
import type { UploadCacheChunkRequest } from "../models/UploadCacheChunkRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class CachesService {
  /**
   * List cache namespaces for one owner scope.
   * @returns CacheNamespaceListApiResponse Namespaces visible to the caller
   * @throws ApiError
   */
  public static listNamespaces({
    ownerType,
    ownerRef,
    namespace,
    freshness,
    limit,
    cursor,
  }: {
    ownerType: OwnerType;
    ownerRef?: string | null;
    /**
     * Case-insensitive namespace substring.
     */
    namespace?: string | null;
    /**
     * Filter by active-generation freshness state.
     */
    freshness?: null | CacheNamespaceFreshness;
    /**
     * Requested page size (bounded server-side).
     */
    limit?: number | null;
    /**
     * Opaque keyset cursor from a prior page.
     */
    cursor?: string | null;
  }): CancelablePromise<CacheNamespaceListApiResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/cache/namespaces",
      query: {
        owner_type: ownerType,
        owner_ref: ownerRef,
        namespace: namespace,
        freshness: freshness,
        limit: limit,
        cursor: cursor,
      },
      errors: {
        400: `Invalid owner selector, filter, limit, or cursor`,
        401: `Authentication required`,
        403: `Cache scope is not accessible`,
        500: `Cache metadata lookup failed`,
      },
    });
  }
  /**
   * Create a cache namespace.
   * @returns CacheNamespaceApiResponse Namespace created
   * @throws ApiError
   */
  public static createNamespace({
    requestBody,
  }: {
    requestBody: CreateCacheNamespaceRequest;
  }): CancelablePromise<CacheNamespaceApiResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/cache/namespaces",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, namespace, or policy`,
        401: `Authentication required`,
        403: `Namespace creation is not permitted`,
        409: `Namespace already exists`,
        500: `Namespace creation failed`,
      },
    });
  }
  /**
   * Show cache namespace metadata and health.
   * @returns CacheNamespaceApiResponse Namespace metadata
   * @throws ApiError
   */
  public static showNamespace({
    namespace,
    ownerType,
    ownerRef,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    /**
     * Owner type: `system`, `identity`, `pack`, `action`, or `sensor`.
     */
    ownerType: OwnerType;
    /**
     * Owner reference (pack/action/sensor ref). Omitted for system scope.
     */
    ownerRef?: string | null;
  }): CancelablePromise<CacheNamespaceApiResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/cache/namespaces/{namespace}",
      path: {
        namespace: namespace,
      },
      query: {
        owner_type: ownerType,
        owner_ref: ownerRef,
      },
      errors: {
        400: `Invalid owner selector or namespace`,
        401: `Authentication required`,
        403: `Namespace is not accessible`,
        404: `Namespace not found`,
        500: `Namespace lookup failed`,
      },
    });
  }
  /**
   * Update a cache namespace's publication policy.
   * @returns CacheNamespaceApiResponse Namespace updated
   * @throws ApiError
   */
  public static updateNamespace({
    namespace,
    requestBody,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    requestBody: UpdateCacheNamespaceRequest;
  }): CancelablePromise<CacheNamespaceApiResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/cache/namespaces/{namespace}",
      path: {
        namespace: namespace,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, namespace, or policy`,
        401: `Authentication required`,
        403: `Namespace update is not permitted`,
        404: `Namespace not found`,
        409: `Namespace is deleted or policy update conflicts`,
        500: `Namespace update failed`,
      },
    });
  }
  /**
   * Tombstone a cache namespace and queue bounded cleanup.
   * @returns CacheNamespaceDeletionApiResponse Namespace tombstoned
   * @throws ApiError
   */
  public static deleteNamespace({
    namespace,
    ownerType,
    ownerRef,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    /**
     * Owner type: `system`, `identity`, `pack`, `action`, or `sensor`.
     */
    ownerType: OwnerType;
    /**
     * Owner reference (pack/action/sensor ref). Omitted for system scope.
     */
    ownerRef?: string | null;
  }): CancelablePromise<CacheNamespaceDeletionApiResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/cache/namespaces/{namespace}",
      path: {
        namespace: namespace,
      },
      query: {
        owner_type: ownerType,
        owner_ref: ownerRef,
      },
      errors: {
        400: `Invalid owner selector or namespace`,
        401: `Authentication required`,
        403: `Namespace deletion is not permitted`,
        404: `Namespace not found`,
        409: `Namespace deletion conflicts with current state`,
        500: `Namespace deletion failed`,
      },
    });
  }
  /**
   * Generation-pinned cursor scan.
   * @returns CacheScanPageApiResponse One scan page
   * @throws ApiError
   */
  public static scanEntries({
    namespace,
    ownerType,
    ownerRef,
    limit,
    requireFresh,
    generation,
    cursor,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    ownerType: OwnerType;
    ownerRef?: string | null;
    /**
     * Requested page size (bounded server-side).
     */
    limit?: number | null;
    requireFresh?: boolean;
    /**
     * Pinned generation. Required together with `cursor` on later pages.
     */
    generation?: null | i64;
    /**
     * Opaque, integrity-protected cursor from a prior page.
     */
    cursor?: string | null;
  }): CancelablePromise<CacheScanPageApiResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/cache/namespaces/{namespace}/entries",
      path: {
        namespace: namespace,
      },
      query: {
        owner_type: ownerType,
        owner_ref: ownerRef,
        limit: limit,
        require_fresh: requireFresh,
        generation: generation,
        cursor: cursor,
      },
      errors: {
        400: `Invalid owner selector, page shape, generation, or cursor`,
        401: `Authentication required`,
        403: `Namespace is not accessible`,
        404: `Namespace not found`,
        409: `Cache is stale, unpopulated, deleted, or the snapshot expired`,
        500: `Cache scan failed`,
      },
    });
  }
  /**
   * Point lookup by external id.
   * @returns CachePointLookupApiResponse Lookup result
   * @throws ApiError
   */
  public static lookupEntry({
    namespace,
    requestBody,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    requestBody: CachePointLookupRequest;
  }): CancelablePromise<CachePointLookupApiResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/cache/namespaces/{namespace}/entries/lookup",
      path: {
        namespace: namespace,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, namespace, or lookup request`,
        401: `Authentication required`,
        403: `Namespace is not accessible`,
        404: `Namespace not found`,
        409: `Cache is stale, unpopulated, deleted, or the snapshot expired`,
        500: `Cache lookup failed`,
      },
    });
  }
  /**
   * Bounded multi-ID lookup.
   * @returns CacheMultiLookupApiResponse Lookup results
   * @throws ApiError
   */
  public static lookupEntries({
    namespace,
    requestBody,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    requestBody: CacheMultiLookupRequest;
  }): CancelablePromise<CacheMultiLookupApiResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/cache/namespaces/{namespace}/entries/lookup-many",
      path: {
        namespace: namespace,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, namespace, or identifier list`,
        401: `Authentication required`,
        403: `Namespace is not accessible`,
        404: `Namespace not found`,
        409: `Cache is stale, unpopulated, deleted, or the snapshot expired`,
        500: `Cache lookup failed`,
      },
    });
  }
  /**
   * List generations for a namespace.
   * @returns CacheGenerationListApiResponse Generations
   * @throws ApiError
   */
  public static listGenerations({
    namespace,
    ownerType,
    ownerRef,
    limit,
    cursor,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    ownerType: OwnerType;
    ownerRef?: string | null;
    /**
     * Requested page size (bounded server-side).
     */
    limit?: number | null;
    /**
     * Opaque keyset cursor from a prior page.
     */
    cursor?: string | null;
  }): CancelablePromise<CacheGenerationListApiResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/cache/namespaces/{namespace}/generations",
      path: {
        namespace: namespace,
      },
      query: {
        owner_type: ownerType,
        owner_ref: ownerRef,
        limit: limit,
        cursor: cursor,
      },
      errors: {
        400: `Invalid owner selector, limit, or cursor`,
        401: `Authentication required`,
        403: `Namespace is not accessible`,
        404: `Namespace not found`,
        500: `Generation metadata lookup failed`,
      },
    });
  }
  /**
   * Begin a staging generation.
   * @returns CacheGenerationApiResponse Matching idempotent generation replay
   * @throws ApiError
   */
  public static createGeneration({
    namespace,
    requestBody,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    requestBody: CreateCacheGenerationRequest;
  }): CancelablePromise<CacheGenerationApiResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/cache/namespaces/{namespace}/generations",
      path: {
        namespace: namespace,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, namespace, or generation request`,
        401: `Authentication required`,
        403: `Generation creation is not permitted`,
        404: `Namespace not found`,
        409: `Refresh id, active-generation precondition, namespace state, or quota conflict`,
        500: `Generation creation failed`,
      },
    });
  }
  /**
   * Show a single generation.
   * @returns CacheGenerationApiResponse Generation
   * @throws ApiError
   */
  public static showGeneration({
    namespace,
    generationId,
    ownerType,
    ownerRef,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    /**
     * Generation id
     */
    generationId: number;
    /**
     * Owner type: `system`, `identity`, `pack`, `action`, or `sensor`.
     */
    ownerType: OwnerType;
    /**
     * Owner reference (pack/action/sensor ref). Omitted for system scope.
     */
    ownerRef?: string | null;
  }): CancelablePromise<CacheGenerationApiResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}",
      path: {
        namespace: namespace,
        generation_id: generationId,
      },
      query: {
        owner_type: ownerType,
        owner_ref: ownerRef,
      },
      errors: {
        400: `Invalid owner selector, namespace, or generation id`,
        401: `Authentication required`,
        403: `Namespace is not accessible`,
        404: `Namespace or generation not found`,
        500: `Generation lookup failed`,
      },
    });
  }
  /**
   * Abandon a staging or ready generation.
   * @returns CacheGenerationApiResponse Abandoned generation
   * @throws ApiError
   */
  public static abandonGeneration({
    namespace,
    generationId,
    requestBody,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    /**
     * Generation id
     */
    generationId: number;
    requestBody: CacheOwnerBody;
  }): CancelablePromise<CacheGenerationApiResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/abandon",
      path: {
        namespace: namespace,
        generation_id: generationId,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, namespace, or generation id`,
        401: `Authentication required`,
        403: `Generation abandonment is not permitted`,
        404: `Namespace or generation not found`,
        409: `Generation cannot be abandoned from its current state`,
        500: `Generation abandonment failed`,
      },
    });
  }
  /**
   * Upload a numbered ingest chunk. Idempotent by generation/chunk index and a
   * server-computed request digest.
   * @returns CacheGenerationApiResponse Chunk accepted or idempotently replayed
   * @throws ApiError
   */
  public static uploadChunk({
    namespace,
    generationId,
    chunkIndex,
    requestBody,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    /**
     * Generation id
     */
    generationId: number;
    /**
     * Zero-based chunk index
     */
    chunkIndex: number;
    requestBody: UploadCacheChunkRequest;
  }): CancelablePromise<CacheGenerationApiResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/chunks/{chunk_index}",
      path: {
        namespace: namespace,
        generation_id: generationId,
        chunk_index: chunkIndex,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, chunk index, or chunk body`,
        401: `Authentication required`,
        403: `Chunk upload is not permitted`,
        404: `Namespace or generation not found`,
        409: `Chunk, generation state, duplicate identifier, or quota conflict`,
        413: `Chunk request exceeds the configured body limit`,
        500: `Chunk upload failed`,
      },
    });
  }
  /**
   * Atomically promote a ready generation.
   * @returns CacheGenerationApiResponse Promoted generation
   * @throws ApiError
   */
  public static promoteGeneration({
    namespace,
    generationId,
    requestBody,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    /**
     * Generation id
     */
    generationId: number;
    requestBody: PromoteCacheGenerationRequest;
  }): CancelablePromise<CacheGenerationApiResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/promote",
      path: {
        namespace: namespace,
        generation_id: generationId,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, namespace, or promotion request`,
        401: `Authentication required`,
        403: `Generation promotion is not permitted`,
        404: `Namespace or generation not found`,
        409: `Promotion state or active-generation precondition failed`,
        500: `Generation promotion failed`,
      },
    });
  }
  /**
   * Seal a staging generation into `ready`.
   * @returns CacheGenerationApiResponse Sealed generation
   * @throws ApiError
   */
  public static sealGeneration({
    namespace,
    generationId,
    requestBody,
  }: {
    /**
     * Cache namespace
     */
    namespace: string;
    /**
     * Generation id
     */
    generationId: number;
    requestBody: SealCacheGenerationRequest;
  }): CancelablePromise<CacheGenerationApiResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/seal",
      path: {
        namespace: namespace,
        generation_id: generationId,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid owner selector, namespace, or seal expectations`,
        401: `Authentication required`,
        403: `Generation sealing is not permitted`,
        404: `Namespace or generation not found`,
        409: `Generation state or seal expectations conflict`,
        500: `Generation sealing failed`,
      },
    });
  }
}
