/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_BuildPackEnvsResponse } from "../models/ApiResponse_BuildPackEnvsResponse";
import type { ApiResponse_DownloadPacksResponse } from "../models/ApiResponse_DownloadPacksResponse";
import type { ApiResponse_GetPackDependenciesResponse } from "../models/ApiResponse_GetPackDependenciesResponse";
import type { ApiResponse_PackInstallResponse } from "../models/ApiResponse_PackInstallResponse";
import type { ApiResponse_PackInstallStatusResponse } from "../models/ApiResponse_PackInstallStatusResponse";
import type { ApiResponse_RegisterPacksResponse } from "../models/ApiResponse_RegisterPacksResponse";
import type { BuildPackEnvsRequest } from "../models/BuildPackEnvsRequest";
import type { CreatePackRegistryIndexRequest } from "../models/CreatePackRegistryIndexRequest";
import type { CreatePackRequest } from "../models/CreatePackRequest";
import type { DownloadPacksRequest } from "../models/DownloadPacksRequest";
import type { GetPackDependenciesRequest } from "../models/GetPackDependenciesRequest";
import type { i64 } from "../models/i64";
import type { InstallPackRequest } from "../models/InstallPackRequest";
import type { PackIndexEntry } from "../models/PackIndexEntry";
import type { PackInstallProvenance } from "../models/PackInstallProvenance";
import type { PackRegistryIndexSummary } from "../models/PackRegistryIndexSummary";
import type { PackResponse } from "../models/PackResponse";
import type { PackTestResult } from "../models/PackTestResult";
import type { PackUploadForm } from "../models/PackUploadForm";
import type { PaginatedResponse_PackSummary } from "../models/PaginatedResponse_PackSummary";
import type { PaginationMeta } from "../models/PaginationMeta";
import type { RegisterPackRequest } from "../models/RegisterPackRequest";
import type { RegisterPacksRequest } from "../models/RegisterPacksRequest";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdatePackRegistryIndexRequest } from "../models/UpdatePackRegistryIndexRequest";
import type { UpdatePackRequest } from "../models/UpdatePackRequest";
import type { Value } from "../models/Value";
import type { WorkflowSyncResult } from "../models/WorkflowSyncResult";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class PacksService {
  /**
   * @returns any Configured pack registry indices
   * @throws ApiError
   */
  public static listPackIndices(): CancelablePromise<{
    data: Array<{
      created: string;
      enabled: boolean;
      headers: Record<string, any>;
      id: number;
      name?: string | null;
      position: number;
      updated: string;
      url: string;
    }>;
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/pack-indices",
      errors: {
        401: `Unauthorized`,
        403: `Forbidden`,
      },
    });
  }
  /**
   * @returns any Pack registry index created
   * @throws ApiError
   */
  public static createPackIndex({
    requestBody,
  }: {
    requestBody: CreatePackRegistryIndexRequest;
  }): CancelablePromise<{
    /**
     * API-managed pack registry index configuration.
     */
    data: {
      created: string;
      enabled: boolean;
      headers: Record<string, any>;
      id: number;
      name?: string | null;
      position: number;
      updated: string;
      url: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/pack-indices",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        401: `Unauthorized`,
        403: `Forbidden`,
      },
    });
  }
  /**
   * @returns any Available indexed packs
   * @throws ApiError
   */
  public static browseIndexedPacks({
    q,
    registryId,
    includeDisabled,
  }: {
    /**
     * Text to match against indexed packs
     */
    q?: string;
    /**
     * Restrict results to a configured registry index
     */
    registryId?: number;
    /**
     * Include disabled registry indices
     */
    includeDisabled?: boolean;
  }): CancelablePromise<{
    data: Array<{
      pack: PackIndexEntry;
      registry: PackRegistryIndexSummary;
    }>;
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/pack-indices/packs",
      query: {
        q: q,
        registry_id: registryId,
        include_disabled: includeDisabled,
      },
      errors: {
        400: `Invalid or disabled selected registry`,
        401: `Unauthorized`,
        403: `Forbidden`,
      },
    });
  }
  /**
   * @returns any Indexed pack
   * @throws ApiError
   */
  public static getIndexedPack({
    ref,
  }: {
    /**
     * Indexed pack reference identifier
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Indexed pack summary with the registry it was resolved from.
     */
    data: {
      pack: PackIndexEntry;
      registry: PackRegistryIndexSummary;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/pack-indices/packs/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        401: `Unauthorized`,
        403: `Forbidden`,
        404: `Indexed pack not found`,
      },
    });
  }
  /**
   * @returns any Pack registry index updated
   * @throws ApiError
   */
  public static updatePackIndex({
    id,
    requestBody,
  }: {
    /**
     * Pack registry index ID
     */
    id: number;
    requestBody: UpdatePackRegistryIndexRequest;
  }): CancelablePromise<{
    /**
     * API-managed pack registry index configuration.
     */
    data: {
      created: string;
      enabled: boolean;
      headers: Record<string, any>;
      id: number;
      name?: string | null;
      position: number;
      updated: string;
      url: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/pack-indices/{id}",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        401: `Unauthorized`,
        403: `Forbidden`,
        404: `Pack registry index not found`,
        409: `Update would reactivate static pack indices`,
      },
    });
  }
  /**
   * @returns SuccessResponse Pack registry index deleted
   * @throws ApiError
   */
  public static deletePackIndex({
    id,
  }: {
    /**
     * Pack registry index ID
     */
    id: number;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/pack-indices/{id}",
      path: {
        id: id,
      },
      errors: {
        401: `Unauthorized`,
        403: `Forbidden`,
        404: `Pack registry index not found`,
        409: `Deletion would reactivate static pack indices`,
      },
    });
  }
  /**
   * List all packs with pagination
   * @returns PaginatedResponse_PackSummary List of packs
   * @throws ApiError
   */
  public static listPacks({
    page,
    pageSize,
    q,
  }: {
    page?: number;
    pageSize?: number;
    /**
     * Keyword query. Tokens are AND-matched across ref, label, and description.
     */
    q?: string | null;
  }): CancelablePromise<PaginatedResponse_PackSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs",
      query: {
        page: page,
        page_size: pageSize,
        q: q,
      },
    });
  }
  /**
   * Create a new pack
   * @returns any Pack created successfully
   * @throws ApiError
   */
  public static createPack({
    requestBody,
  }: {
    requestBody: CreatePackRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for pack information
     */
    data: {
      /**
       * Number of actions registered for this pack
       */
      action_count?: number | null;
      /**
       * Configuration schema
       */
      conf_schema: Record<string, any>;
      /**
       * Pack configuration
       */
      config: Record<string, any>;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Pack dependencies (refs of required packs)
       */
      dependencies: Array<string>;
      /**
       * Pack description
       */
      description?: string | null;
      /**
       * Pack ID
       */
      id: number;
      /**
       * Is standard pack
       */
      is_standard: boolean;
      /**
       * Human-readable label
       */
      label: string;
      /**
       * Pack metadata
       */
      meta: Record<string, any>;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Number of rules registered for this pack
       */
      rule_count?: number | null;
      /**
       * Runtime dependencies (e.g., shell, python, nodejs)
       */
      runtime_deps: Array<string>;
      /**
       * Number of sensors registered for this pack
       */
      sensor_count?: number | null;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Number of triggers registered for this pack
       */
      trigger_count?: number | null;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Pack version
       */
      version: string;
      worker_affinity: Value;
      worker_selector: Value;
      worker_tolerations: Value;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        409: `Pack with same ref already exists`,
      },
    });
  }
  /**
   * Build pack environments
   * @returns ApiResponse_BuildPackEnvsResponse Environments built
   * @throws ApiError
   */
  public static buildPackEnvs({
    requestBody,
  }: {
    requestBody: BuildPackEnvsRequest;
  }): CancelablePromise<ApiResponse_BuildPackEnvsResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/build-envs",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
      },
    });
  }
  /**
   * Get pack dependencies
   * @returns ApiResponse_GetPackDependenciesResponse Dependencies analyzed
   * @throws ApiError
   */
  public static getPackDependencies({
    requestBody,
  }: {
    requestBody: GetPackDependenciesRequest;
  }): CancelablePromise<ApiResponse_GetPackDependenciesResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/dependencies",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
      },
    });
  }
  /**
   * Create pack routes
   * Note: Nested resource routes (e.g., /packs/:ref/actions) are defined
   * in their respective modules (actions.rs, triggers.rs, rules.rs) to avoid
   * route conflicts and maintain proper separation of concerns.
   * Download packs from various sources
   * @returns ApiResponse_DownloadPacksResponse Packs downloaded
   * @throws ApiError
   */
  public static downloadPacks({
    requestBody,
  }: {
    requestBody: DownloadPacksRequest;
  }): CancelablePromise<ApiResponse_DownloadPacksResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/download",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
      },
    });
  }
  /**
   * Install a pack from a Git, archive, local, or managed-registry source.
   * @returns ApiResponse_PackInstallResponse Pack installed successfully
   * @throws ApiError
   */
  public static installPack({
    requestBody,
  }: {
    requestBody: InstallPackRequest;
  }): CancelablePromise<ApiResponse_PackInstallResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/install",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request or tests failed`,
        401: `Unauthorized`,
        403: `Forbidden`,
        404: `Pack or local source not found`,
      },
    });
  }
  /**
   * Get the status of a specific pack install record.
   * @returns ApiResponse_PackInstallStatusResponse Pack install status
   * @throws ApiError
   */
  public static getPackInstall({
    id,
  }: {
    /**
     * Pack install record id
     */
    id: number;
  }): CancelablePromise<ApiResponse_PackInstallStatusResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/install/{id}",
      path: {
        id: id,
      },
      errors: {
        403: `Forbidden`,
        404: `Install record not found`,
      },
    });
  }
  /**
   * Register a pack from local filesystem
   * @returns ApiResponse_PackInstallResponse Pack registered successfully
   * @throws ApiError
   */
  public static registerPack({
    requestBody,
  }: {
    requestBody: RegisterPackRequest;
  }): CancelablePromise<ApiResponse_PackInstallResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/register",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request or tests failed`,
        409: `Pack already exists`,
      },
    });
  }
  /**
   * Register multiple packs
   * @returns ApiResponse_RegisterPacksResponse Packs registered
   * @throws ApiError
   */
  public static registerPacksBatch({
    requestBody,
  }: {
    requestBody: RegisterPacksRequest;
  }): CancelablePromise<ApiResponse_RegisterPacksResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/register-batch",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
      },
    });
  }
  /**
   * Get a single pack test execution by ID
   * @returns any Test execution retrieved
   * @throws ApiError
   */
  public static getPackTest({
    id,
  }: {
    /**
     * Pack test execution id
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Pack test execution record
     */
    data: {
      created: string;
      durationMs: number;
      executionTime: string;
      failed: number;
      id: i64;
      packId: i64;
      packVersion: string;
      passRate: number;
      passed: number;
      result: Value;
      skipped: number;
      totalTests: number;
      triggerReason: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/tests/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Test execution not found`,
      },
    });
  }
  /**
   * Upload and register a pack from a tar.gz archive (multipart/form-data)
   * The archive should be a gzipped tar containing the pack directory at its root
   * (i.e. the archive should unpack to files like `pack.yaml`, `actions/`, etc.).
   * The multipart field name must be `pack`.
   *
   * Optional form fields:
   * - `force`: `"true"` to overwrite an existing pack with the same ref
   * - `skip_tests`: `"true"` to skip test execution after registration
   * @returns any Pack uploaded and registered successfully
   * @throws ApiError
   */
  public static uploadPack({
    formData,
  }: {
    formData: PackUploadForm;
  }): CancelablePromise<{
    /**
     * Response for pack install/register operations with test results
     */
    data: {
      /**
       * ID of the pack install tracking record, present when tests were dispatched.
       */
      install_id?: number | null;
      /**
       * Current install status: pending, running, activating, succeeded, failed, or rolled_back.
       */
      install_status?: string | null;
      /**
       * The installed/registered pack
       */
      pack: PackResponse;
      provenance?: null | PackInstallProvenance;
      test_result?: null | PackTestResult;
      /**
       * Whether tests were skipped
       */
      tests_skipped: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/upload",
      formData: formData,
      mediaType: "multipart/form-data",
      errors: {
        400: `Invalid archive or missing pack.yaml`,
        409: `Pack already exists (use force=true to overwrite)`,
      },
    });
  }
  /**
   * Get a single pack by reference
   * @returns any Pack details
   * @throws ApiError
   */
  public static getPack({
    ref,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Response DTO for pack information
     */
    data: {
      /**
       * Number of actions registered for this pack
       */
      action_count?: number | null;
      /**
       * Configuration schema
       */
      conf_schema: Record<string, any>;
      /**
       * Pack configuration
       */
      config: Record<string, any>;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Pack dependencies (refs of required packs)
       */
      dependencies: Array<string>;
      /**
       * Pack description
       */
      description?: string | null;
      /**
       * Pack ID
       */
      id: number;
      /**
       * Is standard pack
       */
      is_standard: boolean;
      /**
       * Human-readable label
       */
      label: string;
      /**
       * Pack metadata
       */
      meta: Record<string, any>;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Number of rules registered for this pack
       */
      rule_count?: number | null;
      /**
       * Runtime dependencies (e.g., shell, python, nodejs)
       */
      runtime_deps: Array<string>;
      /**
       * Number of sensors registered for this pack
       */
      sensor_count?: number | null;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Number of triggers registered for this pack
       */
      trigger_count?: number | null;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Pack version
       */
      version: string;
      worker_affinity: Value;
      worker_selector: Value;
      worker_tolerations: Value;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Pack not found`,
      },
    });
  }
  /**
   * Update an existing pack
   * @returns any Pack updated successfully
   * @throws ApiError
   */
  public static updatePack({
    ref,
    requestBody,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
    requestBody: UpdatePackRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for pack information
     */
    data: {
      /**
       * Number of actions registered for this pack
       */
      action_count?: number | null;
      /**
       * Configuration schema
       */
      conf_schema: Record<string, any>;
      /**
       * Pack configuration
       */
      config: Record<string, any>;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Pack dependencies (refs of required packs)
       */
      dependencies: Array<string>;
      /**
       * Pack description
       */
      description?: string | null;
      /**
       * Pack ID
       */
      id: number;
      /**
       * Is standard pack
       */
      is_standard: boolean;
      /**
       * Human-readable label
       */
      label: string;
      /**
       * Pack metadata
       */
      meta: Record<string, any>;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Number of rules registered for this pack
       */
      rule_count?: number | null;
      /**
       * Runtime dependencies (e.g., shell, python, nodejs)
       */
      runtime_deps: Array<string>;
      /**
       * Number of sensors registered for this pack
       */
      sensor_count?: number | null;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Number of triggers registered for this pack
       */
      trigger_count?: number | null;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Pack version
       */
      version: string;
      worker_affinity: Value;
      worker_selector: Value;
      worker_tolerations: Value;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/packs/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Pack not found`,
      },
    });
  }
  /**
   * Delete a pack
   * @returns SuccessResponse Pack deleted successfully
   * @throws ApiError
   */
  public static deletePack({
    ref,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/packs/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Pack not found`,
      },
    });
  }
  /**
   * Serve the optional icon bundled at a pack root as `pack-icon.{jpg,png,ico,svg}`.
   * @returns any Pack icon image
   * @throws ApiError
   */
  public static getPackIcon({
    ref,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{ref}/icon",
      path: {
        ref: ref,
      },
      errors: {
        404: `Pack icon not found`,
      },
    });
  }
  /**
   * Get the most recent install status for a pack (survives a rollback).
   * @returns ApiResponse_PackInstallStatusResponse Latest pack install status
   * @throws ApiError
   */
  public static getPackLatestInstall({
    ref,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
  }): CancelablePromise<ApiResponse_PackInstallStatusResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{ref}/install/latest",
      path: {
        ref: ref,
      },
      errors: {
        403: `Forbidden`,
        404: `No install records found for pack`,
      },
    });
  }
  /**
   * Execute tests for a pack
   * @returns any Tests accepted
   * @throws ApiError
   */
  public static testPack({
    ref,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Response for pack install/register operations with test results
     */
    data: {
      /**
       * ID of the pack install tracking record, present when tests were dispatched.
       */
      install_id?: number | null;
      /**
       * Current install status: pending, running, activating, succeeded, failed, or rolled_back.
       */
      install_status?: string | null;
      /**
       * The installed/registered pack
       */
      pack: PackResponse;
      provenance?: null | PackInstallProvenance;
      test_result?: null | PackTestResult;
      /**
       * Whether tests were skipped
       */
      tests_skipped: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/{ref}/test",
      path: {
        ref: ref,
      },
      errors: {
        400: `No enabled pack tests`,
        403: `Forbidden`,
        404: `Pack not found`,
        500: `Test execution failed`,
      },
    });
  }
  /**
   * Get test history for a pack
   * @returns any Test history retrieved
   * @throws ApiError
   */
  public static getPackTestHistory({
    ref,
    page,
    pageSize,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<{
    /**
     * The page items
     */
    items: Array<{
      created: string;
      durationMs: number;
      executionTime: string;
      failed: number;
      id: i64;
      packId: i64;
      packVersion: string;
      passRate: number;
      passed: number;
      result: Value;
      skipped: number;
      totalTests: number;
      triggerReason: string;
    }>;
    /**
     * Pagination metadata
     */
    pagination: PaginationMeta;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{ref}/tests",
      path: {
        ref: ref,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        404: `Pack not found`,
      },
    });
  }
  /**
   * Get latest test result for a pack
   * @returns any Latest test result retrieved
   * @throws ApiError
   */
  public static getPackLatestTest({
    ref,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Pack test execution record
     */
    data: {
      created: string;
      durationMs: number;
      executionTime: string;
      failed: number;
      id: i64;
      packId: i64;
      packVersion: string;
      passRate: number;
      passed: number;
      result: Value;
      skipped: number;
      totalTests: number;
      triggerReason: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{ref}/tests/latest",
      path: {
        ref: ref,
      },
      errors: {
        404: `Pack not found or no tests available`,
      },
    });
  }
  /**
   * Sync workflows from filesystem to database for a pack
   * @returns any Workflows synced successfully
   * @throws ApiError
   */
  public static syncPackWorkflows({
    ref,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Response for pack workflow sync operation
     */
    data: {
      /**
       * Any errors encountered during sync
       */
      errors: Array<string>;
      /**
       * Number of workflows loaded from filesystem
       */
      loaded_count: number;
      /**
       * Pack reference
       */
      pack_ref: string;
      /**
       * Number of workflows registered/updated in database
       */
      registered_count: number;
      /**
       * Individual workflow registration results
       */
      workflows: Array<WorkflowSyncResult>;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/{ref}/workflows/sync",
      path: {
        ref: ref,
      },
      errors: {
        404: `Pack not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Validate workflows for a pack without syncing
   * @returns any Workflows validated
   * @throws ApiError
   */
  public static validatePackWorkflows({
    ref,
  }: {
    /**
     * Pack reference identifier
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Response for pack workflow validation operation
     */
    data: {
      /**
       * Number of workflows with errors
       */
      error_count: number;
      /**
       * Validation errors by workflow reference
       */
      errors: Record<string, Array<string>>;
      /**
       * Pack reference
       */
      pack_ref: string;
      /**
       * Number of workflows validated
       */
      validated_count: number;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/{ref}/workflows/validate",
      path: {
        ref: ref,
      },
      errors: {
        404: `Pack not found`,
        500: `Internal server error`,
      },
    });
  }
}
