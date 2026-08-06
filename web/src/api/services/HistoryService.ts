/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginatedResponse_HistoryRecordResponse } from "../models/PaginatedResponse_HistoryRecordResponse";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class HistoryService {
  /**
   * Get history for a specific execution by ID.
   * Returns all change records for the given execution, ordered by time descending.
   * @returns PaginatedResponse_HistoryRecordResponse History records for the execution
   * @throws ApiError
   */
  public static getExecutionHistory({
    id,
    entityId,
    entityRef,
    operation,
    changedField,
    since,
    until,
    page,
    pageSize,
  }: {
    /**
     * Execution ID
     */
    id: number;
    /**
     * Filter by entity ID
     */
    entityId?: number | null;
    /**
     * Filter by entity ref (e.g., action_ref, worker name)
     */
    entityRef?: string | null;
    /**
     * Filter by operation type: `INSERT`, `UPDATE`, or `DELETE`
     */
    operation?: string | null;
    /**
     * Only include records where this field was changed
     */
    changedField?: string | null;
    /**
     * Only include records at or after this time (ISO 8601)
     */
    since?: string | null;
    /**
     * Only include records at or before this time (ISO 8601)
     */
    until?: string | null;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_HistoryRecordResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/{id}/history",
      path: {
        id: id,
      },
      query: {
        entity_id: entityId,
        entity_ref: entityRef,
        operation: operation,
        changed_field: changedField,
        since: since,
        until: until,
        page: page,
        page_size: pageSize,
      },
    });
  }
  /**
   * List history records for a given entity type.
   * Supported entity types: `execution`, `worker`.
   * Returns a paginated list of change records ordered by time descending.
   * @returns PaginatedResponse_HistoryRecordResponse Paginated list of history records
   * @throws ApiError
   */
  public static listEntityHistory({
    entityType,
    entityId,
    entityRef,
    operation,
    changedField,
    since,
    until,
    page,
    pageSize,
  }: {
    /**
     * Entity type: execution or worker
     */
    entityType: string;
    /**
     * Filter by entity ID
     */
    entityId?: number | null;
    /**
     * Filter by entity ref (e.g., action_ref, worker name)
     */
    entityRef?: string | null;
    /**
     * Filter by operation type: `INSERT`, `UPDATE`, or `DELETE`
     */
    operation?: string | null;
    /**
     * Only include records where this field was changed
     */
    changedField?: string | null;
    /**
     * Only include records at or after this time (ISO 8601)
     */
    since?: string | null;
    /**
     * Only include records at or before this time (ISO 8601)
     */
    until?: string | null;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_HistoryRecordResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/history/{entity_type}",
      path: {
        entity_type: entityType,
      },
      query: {
        entity_id: entityId,
        entity_ref: entityRef,
        operation: operation,
        changed_field: changedField,
        since: since,
        until: until,
        page: page,
        page_size: pageSize,
      },
      errors: {
        400: `Invalid entity type`,
      },
    });
  }
  /**
   * Get history for a specific worker by ID.
   * Returns all change records for the given worker, ordered by time descending.
   * @returns PaginatedResponse_HistoryRecordResponse History records for the worker
   * @throws ApiError
   */
  public static getWorkerHistory({
    id,
    entityId,
    entityRef,
    operation,
    changedField,
    since,
    until,
    page,
    pageSize,
  }: {
    /**
     * Worker ID
     */
    id: number;
    /**
     * Filter by entity ID
     */
    entityId?: number | null;
    /**
     * Filter by entity ref (e.g., action_ref, worker name)
     */
    entityRef?: string | null;
    /**
     * Filter by operation type: `INSERT`, `UPDATE`, or `DELETE`
     */
    operation?: string | null;
    /**
     * Only include records where this field was changed
     */
    changedField?: string | null;
    /**
     * Only include records at or after this time (ISO 8601)
     */
    since?: string | null;
    /**
     * Only include records at or before this time (ISO 8601)
     */
    until?: string | null;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_HistoryRecordResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/workers/{id}/history",
      path: {
        id: id,
      },
      query: {
        entity_id: entityId,
        entity_ref: entityRef,
        operation: operation,
        changed_field: changedField,
        since: since,
        until: until,
        page: page,
        page_size: pageSize,
      },
    });
  }
}
