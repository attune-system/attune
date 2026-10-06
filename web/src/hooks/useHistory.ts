import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { apiClient } from "@/lib/api-client";

/**
 * Supported entity types for history queries.
 * Maps to the PostgreSQL history tables.
 */
export type HistoryEntityType = "execution" | "worker";

/**
 * A single history record from the API.
 */
export interface HistoryRecord {
  /** When the change occurred */
  time: string;
  /** The operation: INSERT, UPDATE, or DELETE */
  operation: string;
  /** The primary key of the changed entity */
  entity_id: number;
  /** Denormalized human-readable identifier (e.g., action_ref, worker name) */
  entity_ref: string | null;
  /** Names of fields that changed */
  changed_fields: string[];
  /** Previous values of changed fields (null for INSERT) */
  old_values: Record<string, unknown> | null;
  /** New values of changed fields (null for DELETE) */
  new_values: Record<string, unknown> | null;
}

/**
 * Paginated history response from the API.
 */
export interface PaginatedHistoryResponse {
  items: HistoryRecord[];
  pagination: {
    page: number;
    page_size: number;
    total_items: number;
    total_pages: number;
  };
}

/**
 * Query parameters for history requests.
 */
export interface HistoryQueryParams {
  /** Filter by operation type */
  operation?: string;
  /** Only include records where this field was changed */
  changed_field?: string;
  /** Only include records at or after this time (ISO 8601) */
  since?: string;
  /** Only include records at or before this time (ISO 8601) */
  until?: string;
  /** Page number (1-based) */
  page?: number;
  /** Number of items per page */
  page_size?: number;
}

/**
 * Fetch history for a specific entity by its type and ID.
 *
 * Uses the entity-specific endpoints:
 * - GET /api/v1/executions/:id/history
 * - GET /api/v1/workers/:id/history
 */
async function fetchEntityHistory(
  entityType: HistoryEntityType,
  entityId: number,
  params: HistoryQueryParams,
): Promise<PaginatedHistoryResponse> {
  const pluralMap: Record<HistoryEntityType, string> = {
    execution: "executions",
    worker: "workers",
  };

  const queryParams: Record<string, string | number> = {};
  if (params.operation) queryParams.operation = params.operation;
  if (params.changed_field) queryParams.changed_field = params.changed_field;
  if (params.since) queryParams.since = params.since;
  if (params.until) queryParams.until = params.until;
  if (params.page) queryParams.page = params.page;
  if (params.page_size) queryParams.page_size = params.page_size;

  const response = await apiClient.get<PaginatedHistoryResponse>(
    `/api/v1/${pluralMap[entityType]}/${entityId}/history`,
    { params: queryParams },
  );

  return response.data;
}

/**
 * React Query hook for fetching entity history.
 *
 * @param entityType - The type of entity (execution, worker, enforcement, event)
 * @param entityId - The entity's primary key
 * @param params - Optional query parameters for filtering and pagination
 * @param enabled - Whether the query should execute (default: true when entityId is truthy)
 */
export function useEntityHistory(
  entityType: HistoryEntityType,
  entityId: number,
  params: HistoryQueryParams = {},
  enabled?: boolean,
) {
  const isEnabled = enabled ?? !!entityId;

  return useQuery({
    queryKey: ["history", entityType, entityId, params],
    queryFn: () => fetchEntityHistory(entityType, entityId, params),
    enabled: isEnabled,
    staleTime: 30000,
    placeholderData: keepPreviousData,
  });
}

/**
 * Convenience hook for execution history.
 */
export function useExecutionHistory(
  executionId: number,
  params: HistoryQueryParams = {},
) {
  return useEntityHistory("execution", executionId, params);
}

/**
 * Convenience hook for worker history.
 */
export function useWorkerHistory(
  workerId: number,
  params: HistoryQueryParams = {},
) {
  return useEntityHistory("worker", workerId, params);
}
