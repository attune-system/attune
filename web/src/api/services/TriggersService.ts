/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_TriggerResponse } from "../models/ApiResponse_TriggerResponse";
import type { CreateTriggerRequest } from "../models/CreateTriggerRequest";
import type { PaginatedResponse_TriggerSummary } from "../models/PaginatedResponse_TriggerSummary";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateTriggerRequest } from "../models/UpdateTriggerRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class TriggersService {
  /**
   * List triggers by pack reference
   * @returns PaginatedResponse_TriggerSummary List of triggers in pack
   * @throws ApiError
   */
  public static listTriggersByPack({
    packRef,
    page,
    pageSize,
    q,
  }: {
    /**
     * Pack reference
     */
    packRef: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    /**
     * Keyword query. Whitespace-separated tokens are AND-matched against
     * the catalog item's discovery fields.
     */
    q?: string | null;
  }): CancelablePromise<PaginatedResponse_TriggerSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/triggers",
      path: {
        pack_ref: packRef,
      },
      query: {
        page: page,
        page_size: pageSize,
        q: q,
      },
      errors: {
        404: `Pack not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * List all triggers with pagination
   * @returns PaginatedResponse_TriggerSummary List of triggers
   * @throws ApiError
   */
  public static listTriggers({
    page,
    pageSize,
    q,
    referencingPackRef,
  }: {
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    /**
     * Keyword query. Tokens are AND-matched across ref, label, description,
     * and pack_ref.
     */
    q?: string | null;
    /**
     * Optional pack ref that wants to subscribe to the returned triggers.
     */
    referencingPackRef?: string | null;
  }): CancelablePromise<PaginatedResponse_TriggerSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/triggers",
      query: {
        page: page,
        page_size: pageSize,
        q: q,
        referencing_pack_ref: referencingPackRef,
      },
      errors: {
        500: `Internal server error`,
      },
    });
  }
  /**
   * Create a new trigger
   * @returns ApiResponse_TriggerResponse Trigger created successfully
   * @throws ApiError
   */
  public static createTrigger({
    requestBody,
  }: {
    requestBody: CreateTriggerRequest;
  }): CancelablePromise<ApiResponse_TriggerResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/triggers",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        404: `Pack not found`,
        409: `Trigger with same ref already exists`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * List enabled triggers
   * @returns PaginatedResponse_TriggerSummary List of enabled triggers
   * @throws ApiError
   */
  public static listEnabledTriggers({
    page,
    pageSize,
    q,
    referencingPackRef,
  }: {
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    /**
     * Keyword query. Tokens are AND-matched across ref, label, description,
     * and pack_ref.
     */
    q?: string | null;
    /**
     * Optional pack ref that wants to subscribe to the returned triggers.
     */
    referencingPackRef?: string | null;
  }): CancelablePromise<PaginatedResponse_TriggerSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/triggers/enabled",
      query: {
        page: page,
        page_size: pageSize,
        q: q,
        referencing_pack_ref: referencingPackRef,
      },
      errors: {
        500: `Internal server error`,
      },
    });
  }
  /**
   * Get a single trigger by reference
   * @returns ApiResponse_TriggerResponse Trigger details
   * @throws ApiError
   */
  public static getTrigger({
    ref,
    referencingPackRef,
  }: {
    /**
     * Trigger reference
     */
    ref: string;
    /**
     * Optional pack ref that wants to subscribe to this trigger.
     */
    referencingPackRef?: string | null;
  }): CancelablePromise<ApiResponse_TriggerResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/triggers/{ref}",
      path: {
        ref: ref,
      },
      query: {
        referencing_pack_ref: referencingPackRef,
      },
      errors: {
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Update an existing trigger
   * @returns ApiResponse_TriggerResponse Trigger updated successfully
   * @throws ApiError
   */
  public static updateTrigger({
    ref,
    requestBody,
  }: {
    /**
     * Trigger reference
     */
    ref: string;
    requestBody: UpdateTriggerRequest;
  }): CancelablePromise<ApiResponse_TriggerResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/triggers/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Delete a trigger
   * @returns SuccessResponse Trigger deleted successfully
   * @throws ApiError
   */
  public static deleteTrigger({
    ref,
  }: {
    /**
     * Trigger reference
     */
    ref: string;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/triggers/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Disable a trigger
   * @returns ApiResponse_TriggerResponse Trigger disabled successfully
   * @throws ApiError
   */
  public static disableTrigger({
    ref,
  }: {
    /**
     * Trigger reference
     */
    ref: string;
  }): CancelablePromise<ApiResponse_TriggerResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/triggers/{ref}/disable",
      path: {
        ref: ref,
      },
      errors: {
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Enable a trigger
   * @returns ApiResponse_TriggerResponse Trigger enabled successfully
   * @throws ApiError
   */
  public static enableTrigger({
    ref,
  }: {
    /**
     * Trigger reference
     */
    ref: string;
  }): CancelablePromise<ApiResponse_TriggerResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/triggers/{ref}/enable",
      path: {
        ref: ref,
      },
      errors: {
        404: `Trigger not found`,
        500: `Internal server error`,
      },
    });
  }
}
