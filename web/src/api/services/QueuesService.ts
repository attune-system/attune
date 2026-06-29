/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_WorkQueueItemResponse } from "../models/ApiResponse_WorkQueueItemResponse";
import type { ApiResponse_WorkQueueResponse } from "../models/ApiResponse_WorkQueueResponse";
import type { CreateWorkQueueRequest } from "../models/CreateWorkQueueRequest";
import type { EnqueueWorkQueueItemRequest } from "../models/EnqueueWorkQueueItemRequest";
import type { PaginatedResponse_WorkQueueItemResponse } from "../models/PaginatedResponse_WorkQueueItemResponse";
import type { PaginatedResponse_WorkQueueSummary } from "../models/PaginatedResponse_WorkQueueSummary";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateWorkQueueItemRequest } from "../models/UpdateWorkQueueItemRequest";
import type { UpdateWorkQueueRequest } from "../models/UpdateWorkQueueRequest";
import type { WorkQueueItemStatus } from "../models/WorkQueueItemStatus";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class QueuesService {
  /**
   * @returns PaginatedResponse_WorkQueueSummary List of work queue definitions for a pack
   * @throws ApiError
   */
  public static listQueuesByPack({
    packRef,
    enabled,
    isAdhoc,
    search,
    referencingPackRef,
    page,
    perPage,
  }: {
    /**
     * Pack reference identifier
     */
    packRef: string;
    enabled?: boolean | null;
    isAdhoc?: boolean | null;
    search?: string | null;
    referencingPackRef?: string | null;
    page?: number;
    perPage?: number;
  }): CancelablePromise<PaginatedResponse_WorkQueueSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/queues",
      path: {
        pack_ref: packRef,
      },
      query: {
        enabled: enabled,
        is_adhoc: isAdhoc,
        search: search,
        referencing_pack_ref: referencingPackRef,
        page: page,
        per_page: perPage,
      },
      errors: {
        403: `Insufficient permissions`,
        404: `Pack not found`,
      },
    });
  }
  /**
   * @returns PaginatedResponse_WorkQueueSummary List of work queue definitions
   * @throws ApiError
   */
  public static listQueues({
    enabled,
    isAdhoc,
    search,
    referencingPackRef,
    page,
    perPage,
  }: {
    enabled?: boolean | null;
    isAdhoc?: boolean | null;
    search?: string | null;
    referencingPackRef?: string | null;
    page?: number;
    perPage?: number;
  }): CancelablePromise<PaginatedResponse_WorkQueueSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/queues",
      query: {
        enabled: enabled,
        is_adhoc: isAdhoc,
        search: search,
        referencing_pack_ref: referencingPackRef,
        page: page,
        per_page: perPage,
      },
    });
  }
  /**
   * @returns ApiResponse_WorkQueueResponse Work queue created successfully
   * @throws ApiError
   */
  public static createQueue({
    requestBody,
  }: {
    requestBody: CreateWorkQueueRequest;
  }): CancelablePromise<ApiResponse_WorkQueueResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/queues",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        403: `Insufficient permissions`,
        404: `Pack or dispatch action not found`,
        409: `Queue with same ref already exists`,
      },
    });
  }
  /**
   * @returns ApiResponse_WorkQueueResponse Work queue definition
   * @throws ApiError
   */
  public static getQueue({
    ref,
    referencingPackRef,
  }: {
    /**
     * Queue reference identifier
     */
    ref: string;
    referencingPackRef?: string | null;
  }): CancelablePromise<ApiResponse_WorkQueueResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/queues/{ref}",
      path: {
        ref: ref,
      },
      query: {
        referencing_pack_ref: referencingPackRef,
      },
      errors: {
        404: `Queue not found`,
      },
    });
  }
  /**
   * @returns ApiResponse_WorkQueueResponse Work queue updated successfully
   * @throws ApiError
   */
  public static updateQueue({
    ref,
    requestBody,
  }: {
    /**
     * Queue reference identifier
     */
    ref: string;
    requestBody: UpdateWorkQueueRequest;
  }): CancelablePromise<ApiResponse_WorkQueueResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/queues/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        403: `Insufficient permissions or pack-managed queue`,
        404: `Queue, pack, or dispatch action not found`,
      },
    });
  }
  /**
   * @returns SuccessResponse Work queue deleted successfully
   * @throws ApiError
   */
  public static deleteQueue({
    ref,
  }: {
    /**
     * Queue reference identifier
     */
    ref: string;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/queues/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        403: `Insufficient permissions or pack-managed queue`,
        404: `Queue not found`,
      },
    });
  }
  /**
   * @returns PaginatedResponse_WorkQueueItemResponse List of queue items
   * @throws ApiError
   */
  public static listQueueItems({
    ref,
    itemKey,
    enqueueSource,
    statuses,
    page,
    perPage,
  }: {
    /**
     * Queue reference identifier
     */
    ref: string;
    itemKey: string | null;
    enqueueSource: string | null;
    statuses: Array<WorkQueueItemStatus>;
    page: number;
    perPage: number;
  }): CancelablePromise<PaginatedResponse_WorkQueueItemResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/queues/{ref}/items",
      path: {
        ref: ref,
        item_key: itemKey,
        enqueue_source: enqueueSource,
        statuses: statuses,
        page: page,
        per_page: perPage,
      },
      errors: {
        403: `Insufficient permissions`,
        404: `Queue not found`,
      },
    });
  }
  /**
   * @returns ApiResponse_WorkQueueItemResponse Pending queue item updated
   * @throws ApiError
   */
  public static enqueueQueueItem({
    ref,
    requestBody,
  }: {
    /**
     * Queue reference identifier
     */
    ref: string;
    requestBody: EnqueueWorkQueueItemRequest;
  }): CancelablePromise<ApiResponse_WorkQueueItemResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/queues/{ref}/items",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        403: `Insufficient permissions`,
        404: `Queue not found`,
        409: `Pending item conflict`,
      },
    });
  }
  /**
   * @returns ApiResponse_WorkQueueItemResponse Queue item updated
   * @throws ApiError
   */
  public static updateQueueItem({
    ref,
    itemId,
    requestBody,
  }: {
    /**
     * Queue reference identifier
     */
    ref: string;
    /**
     * Queue item identifier
     */
    itemId: number;
    requestBody: UpdateWorkQueueItemRequest;
  }): CancelablePromise<ApiResponse_WorkQueueItemResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/queues/{ref}/items/{item_id}",
      path: {
        ref: ref,
        item_id: itemId,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        403: `Insufficient permissions`,
        404: `Queue or queue item not found`,
        409: `Queue item is not mutable`,
      },
    });
  }
  /**
   * @returns SuccessResponse Queue item deleted
   * @throws ApiError
   */
  public static deleteQueueItem({
    ref,
    itemId,
  }: {
    /**
     * Queue reference identifier
     */
    ref: string;
    /**
     * Queue item identifier
     */
    itemId: number;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/queues/{ref}/items/{item_id}",
      path: {
        ref: ref,
        item_id: itemId,
      },
      errors: {
        403: `Insufficient permissions`,
        404: `Queue or queue item not found`,
        409: `Queue item is not mutable`,
      },
    });
  }
}
