import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  useEntityNotifications,
  type Notification,
} from "@/contexts/WebSocketContext";
import type { ExecutionSummary, ExecutionStatus } from "@/api";
import { matchesRefFilter } from "@/utils/refFilters";

interface UseExecutionStreamOptions {
  /**
   * Optional execution ID to filter updates for a specific execution.
   * If not provided, receives updates for all executions.
   */
  executionId?: number;

  /**
   * Whether the stream should be active.
   * Defaults to true.
   */
  enabled?: boolean;

  /**
   * Whether live list updates should be paused.
   * Detail-query updates are also paused when this is true.
   */
  paused?: boolean;

  /**
   * Maximum number of records retained in live-updated list caches.
   * Defaults to 100 for top-level execution lists. Child execution queries are
   * intentionally not capped because workflow views need complete task lists.
   */
  maxListItems?: number;
}

/**
 * Notification metadata fields that come from the PostgreSQL trigger payload
 * but are NOT part of the ExecutionSummary API model. These are stripped
 * before storing execution data in the React Query cache.
 */
const NOTIFICATION_META_FIELDS = [
  "entity_type",
  "entity_id",
  "old_status",
  "action_id",
] as const;

/**
 * Statuses for which the execution has reached a terminal state and the
 * server-side `result` is now final. The NOTIFY payload does not include
 * `result` (it's potentially large JSONB), so we must refetch the execution
 * detail to display it.
 */
const TERMINAL_STATUSES: ReadonlySet<string> = new Set([
  "completed",
  "failed",
  "cancelled",
  "timeout",
  "abandoned",
]);

/** Shape of data coming from WebSocket notifications for executions */
interface ExecutionNotification {
  entity_id: number;
  entity_type: string;
  notification_type: string;
  payload: ExecutionNotificationPayload;
  timestamp: string;
}

/** The raw payload from the PostgreSQL trigger, which includes extra meta fields */
interface ExecutionNotificationPayload extends Partial<ExecutionSummary> {
  entity_type?: string;
  entity_id?: number;
  old_status?: string;
  action_id?: number;
}

/** Query params shape used in execution list query keys */
interface ExecutionQueryParams {
  topLevelOnly?: boolean;
  parent?: number;
  status?: string;
  actionRef?: string;
  executor?: number;
  ruleRef?: string;
  triggerRef?: string;
}

/** Shape of the paginated API response stored in React Query cache */
interface ExecutionListCache {
  items: ExecutionSummary[];
  pagination?: {
    total_items?: number;
    total_pages?: number;
    page?: number;
    page_size?: number;
    has_previous?: boolean;
    has_next?: boolean;
  };
}

/** Shape of a single execution detail response stored in React Query cache */
interface ExecutionDetailCache {
  data: ExecutionSummary;
}

/**
 * Strip notification-only metadata fields from the payload so cached data
 * matches the shape returned by the API (ExecutionSummary / ExecutionResponse).
 */
function stripNotificationMeta(
  payload: ExecutionNotificationPayload,
): Partial<ExecutionSummary> {
  if (!payload || typeof payload !== "object") return payload;
  const cleaned = { ...payload };
  for (const key of NOTIFICATION_META_FIELDS) {
    delete cleaned[key];
  }
  return cleaned;
}

/**
 * Check if an execution matches the given query parameters.
 * Only checks fields that are reliably present in WebSocket payloads.
 */
function executionMatchesParams(
  execution: Partial<ExecutionSummary> & { parent?: number | null },
  params: ExecutionQueryParams | undefined,
): boolean {
  if (!params) return true;

  // Check topLevelOnly filter — child executions (with a parent) must not
  // appear in top-level list queries.
  if (params.topLevelOnly && execution.parent != null) {
    return false;
  }

  // Check parent filter — child execution queries (keyed by { parent: id })
  // should only receive notifications for executions belonging to that parent.
  // Without this, every execution notification would match child queries since
  // they have no other filter fields.
  if (params.parent !== undefined) {
    if (execution.parent !== params.parent) {
      return false;
    }
  }

  // Check status filter (from API query parameters)
  if (params.status && execution.status !== params.status) {
    return false;
  }

  // Check action filter. Supports exact refs and glob-style `*` wildcards.
  if (!matchesRefFilter(execution.action_ref, params.actionRef)) {
    return false;
  }

  // Note: executor is not part of ExecutionSummary so we cannot filter on it
  // from WebSocket payloads. Executor-filtered queries rely on API refetch.

  // Check rule filter. Execution notifications include rule_ref from the
  // execution/enforcement trigger payload, so rule-filtered lists can be
  // updated incrementally from the WebSocket stream.
  if (!matchesRefFilter(execution.rule_ref, params.ruleRef)) {
    return false;
  }

  // Check trigger filter for the same reason as rule_ref.
  if (!matchesRefFilter(execution.trigger_ref, params.triggerRef)) {
    return false;
  }

  return true;
}

/**
 * Check if query params include filters not present in WebSocket payloads.
 */
function hasUnsupportedFilters(
  params: ExecutionQueryParams | undefined,
): boolean {
  if (!params) return false;
  return !!params.executor;
}

/**
 * Hook to subscribe to real-time execution updates via WebSocket.
 *
 * Automatically reconnects on connection loss and updates React Query cache
 * when execution updates are received.
 *
 * @example
 * ```tsx
 * // Listen to all execution updates
 * useExecutionStream();
 *
 * // Listen to updates for a specific execution
 * useExecutionStream({ executionId: 123 });
 * ```
 */
export function useExecutionStream(options: UseExecutionStreamOptions = {}) {
  const {
    executionId,
    enabled = true,
    paused = false,
    maxListItems = 100,
  } = options;
  const queryClient = useQueryClient();

  const handleNotification = useCallback(
    (notification: Notification) => {
      if (paused) {
        return;
      }

      const executionNotification =
        notification as unknown as ExecutionNotification;
      // Filter by execution ID if specified
      if (executionId && executionNotification.entity_id !== executionId) {
        return;
      }

      // Extract execution data from notification payload (flat structure).
      // Keep raw payload for old_status inspection, but use cleaned data for cache.
      const rawPayload = executionNotification.payload;
      const oldStatus: string | undefined = rawPayload?.old_status;
      const executionData = stripNotificationMeta(rawPayload);

      // Update specific execution query if it exists
      queryClient.setQueryData(
        ["executions", executionNotification.entity_id],
        (old: ExecutionDetailCache | undefined) => {
          if (!old) return old;
          return {
            ...old,
            data: {
              ...old.data,
              ...executionData,
            },
          };
        },
      );

      queryClient.invalidateQueries({
        queryKey: ["history", "execution", executionNotification.entity_id],
        exact: false,
      });

      // The NOTIFY payload omits `result` (large JSONB). When the execution
      // transitions into a terminal status, refetch the detail query so the
      // UI can promptly display the final result/output.
      const newStatus = executionData.status;
      const becameTerminal =
        typeof newStatus === "string" &&
        TERMINAL_STATUSES.has(newStatus) &&
        oldStatus !== newStatus;
      if (becameTerminal) {
        queryClient.invalidateQueries({
          queryKey: ["executions", executionNotification.entity_id],
          exact: true,
        });
        queryClient.invalidateQueries({
          queryKey: ["artifacts", "execution", executionNotification.entity_id],
          exact: true,
        });
      }

      // Update execution list queries by modifying existing data.
      // We need to iterate manually to access query keys for filtering.
      const queries = queryClient
        .getQueriesData<ExecutionListCache>({
          queryKey: ["executions"],
          exact: false,
        })
        .filter(([, data]) => data && Array.isArray(data?.items));

      queries.forEach(([queryKey, oldData]) => {
        // Extract query params from the query key (format: ["executions", params])
        const queryParams = queryKey[1] as ExecutionQueryParams | undefined;

        // Child execution queries (keyed by { parent: id }) fetch all pages
        // and must not be capped — the timeline DAG needs every child.
        const isChildQuery = !!queryParams?.parent;

        const old = oldData as ExecutionListCache;

        // Check if execution already exists in the list
        const existingIndex = old.items.findIndex(
          (exec) => exec.id === executionNotification.entity_id,
        );

        // Merge the updated fields to determine if the execution matches the query
        const mergedExecution =
          existingIndex >= 0
            ? { ...old.items[existingIndex], ...executionData }
            : (executionData as ExecutionSummary);
        const matchesQuery = executionMatchesParams(
          mergedExecution,
          queryParams,
        );

        let updatedData: ExecutionSummary[];
        let totalItemsDelta = 0;

        if (existingIndex >= 0) {
          // ── Execution IS in the local data array ──
          if (matchesQuery) {
            // Still matches — update in place, no total_items change
            updatedData = [...old.items];
            updatedData[existingIndex] = {
              ...updatedData[existingIndex],
              ...executionData,
            };
          } else {
            // No longer matches the query filter — remove it
            updatedData = old.items.filter((_, i) => i !== existingIndex);
            totalItemsDelta = -1;
          }
        } else {
          // ── Execution is NOT in the local data array ──
          // This happens when the execution is beyond the fetched page boundary
          // (e.g., running count query with pageSize=1) or was pushed out by
          // the 50-item cap after many new executions were prepended.

          if (oldStatus) {
            // This is a status-change notification (has old_status from the
            // PostgreSQL trigger). Use old_status to detect whether the
            // execution crossed a query filter boundary — even though it's
            // not in our local data array, total_items must stay accurate.
            const virtualOldExecution = {
              ...mergedExecution,
              status: oldStatus as ExecutionStatus,
            };
            const oldMatchedQuery = executionMatchesParams(
              virtualOldExecution,
              queryParams,
            );

            if (oldMatchedQuery && !matchesQuery) {
              // Execution LEFT this query's result set (e.g., was running,
              // now completed). Decrement total_items but don't touch the
              // data array — the item was never in it.
              updatedData = old.items;
              totalItemsDelta = -1;
            } else if (!oldMatchedQuery && matchesQuery) {
              // Execution ENTERED this query's result set.
              if (hasUnsupportedFilters(queryParams)) {
                return;
              }
              updatedData = isChildQuery
                ? [...old.items, executionData as ExecutionSummary]
                : [executionData as ExecutionSummary, ...old.items].slice(
                    0,
                    maxListItems,
                  );
              totalItemsDelta = 1;
            } else {
              // No boundary crossing: either both match (execution was
              // already counted in total_items — don't double-count) or
              // neither matches (irrelevant to this query).
              return;
            }
          } else {
            // No old_status: this is likely an execution_created notification
            // (INSERT trigger). Use the standard add-if-matches logic.
            if (hasUnsupportedFilters(queryParams)) {
              return;
            }

            if (matchesQuery) {
              // Add to the list. Child queries keep all items (no cap);
              // other lists cap at 50 to prevent unbounded growth.
              updatedData = isChildQuery
                ? [...old.items, executionData as ExecutionSummary]
                : [executionData as ExecutionSummary, ...old.items].slice(
                    0,
                    maxListItems,
                  );
              totalItemsDelta = 1;
            } else {
              return;
            }
          }
        }

        // Update the query with the new data
        const page = old.pagination?.page ?? 1;
        const pageSize =
          old.pagination?.page_size ??
          (isChildQuery ? old.items.length : maxListItems);
        const hasExactTotal = old.pagination?.total_items != null;
        const nextPagination = old.pagination
          ? { ...old.pagination }
          : undefined;

        if (nextPagination) {
          nextPagination.has_previous = page > 1;

          if (hasExactTotal) {
            const newTotal = Math.max(
              0,
              (old.pagination?.total_items ?? 0) + totalItemsDelta,
            );
            nextPagination.total_items = newTotal;
            nextPagination.total_pages =
              pageSize > 0 ? Math.ceil(newTotal / pageSize) : 0;
            nextPagination.has_next = page * pageSize < newTotal;
          } else if (
            !isChildQuery &&
            totalItemsDelta > 0 &&
            old.items.length >= pageSize
          ) {
            nextPagination.has_next = true;
          }
        }

        queryClient.setQueryData(queryKey, {
          ...old,
          items: updatedData,
          pagination: nextPagination,
        });
      });
    },
    [executionId, maxListItems, paused, queryClient],
  );

  const { connected } = useEntityNotifications(
    "execution",
    handleNotification,
    enabled && !paused,
  );

  return {
    isConnected: connected,
    error: null,
  };
}
