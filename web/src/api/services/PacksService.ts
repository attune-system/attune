/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_PackInstallResponse } from "../models/ApiResponse_PackInstallResponse";
import type { CreatePackRequest } from "../models/CreatePackRequest";
import type { i64 } from "../models/i64";
import type { InstallPackRequest } from "../models/InstallPackRequest";
import type { PaginatedResponse_PackSummary } from "../models/PaginatedResponse_PackSummary";
import type { PaginationMeta } from "../models/PaginationMeta";
import type { RegisterPackRequest } from "../models/RegisterPackRequest";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { TestSuiteResult } from "../models/TestSuiteResult";
import type { UpdatePackRequest } from "../models/UpdatePackRequest";
import type { Value } from "../models/Value";
import type { WorkflowSyncResult } from "../models/WorkflowSyncResult";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class PacksService {
  /**
   * List all packs with pagination
   * @returns PaginatedResponse_PackSummary List of packs
   * @throws ApiError
   */
  public static listPacks({
    page,
    pageSize,
  }: {
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_PackSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs",
      query: {
        page: page,
        page_size: pageSize,
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
       * Runtime dependencies (e.g., shell, python, nodejs)
       */
      runtime_deps: Array<string>;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Pack version
       */
      version: string;
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
   * Install a pack from remote source (git repository)
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
        501: `Not implemented yet`,
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
       * Runtime dependencies (e.g., shell, python, nodejs)
       */
      runtime_deps: Array<string>;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Pack version
       */
      version: string;
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
       * Runtime dependencies (e.g., shell, python, nodejs)
       */
      runtime_deps: Array<string>;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Pack version
       */
      version: string;
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
   * Execute tests for a pack
   * @returns any Tests executed successfully
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
     * Pack test result structure (not from DB, used for test execution)
     */
    data: {
      durationMs: number;
      executionTime: string;
      failed: number;
      packRef: string;
      packVersion: string;
      passRate: number;
      passed: number;
      skipped: number;
      status: string;
      testSuites: Array<TestSuiteResult>;
      totalTests: number;
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
  }): CancelablePromise<any> {
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
