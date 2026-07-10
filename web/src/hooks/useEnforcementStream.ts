import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  useEntityNotifications,
  type Notification,
} from "@/contexts/WebSocketContext";
import type { EnforcementSummary } from "@/api";

interface UseEnforcementStreamOptions {
  /**
   * Optional enforcement ID to filter updates for a specific enforcement.
   * If not provided, receives updates for all enforcements.
   */
  enforcementId?: number;

  /**
   * Whether the stream should be active.
   * Defaults to true.
   */
  enabled?: boolean;

  /**
   * Whether live updates should be paused for this view.
   */
  paused?: boolean;

  /**
   * Maximum number of records retained in live-updated list caches.
   * Defaults to 100.
   */
  maxListItems?: number;
}

/** Shape of data coming from WebSocket notifications for enforcements */
interface EnforcementNotification {
  entity_id: number;
  entity_type: string;
  notification_type: string;
  payload: Partial<EnforcementSummary> & Record<string, unknown>;
  timestamp: string;
}

/** Query params shape used in enforcement list query keys */
interface EnforcementQueryParams {
  status?: string;
  event?: number;
  rule?: number;
  triggerRef?: string;
  ruleRef?: string;
}

/** Shape of the paginated API response stored in React Query cache */
interface EnforcementListCache {
  items: EnforcementSummary[];
  pagination?: {
    total_items?: number;
    total_pages?: number;
    page?: number;
    page_size?: number;
    has_previous?: boolean;
    has_next?: boolean;
  };
}

/** Shape of a single enforcement detail response stored in React Query cache */
interface EnforcementDetailCache {
  data: EnforcementSummary;
}

/**
 * Check if an enforcement matches the given query parameters
 * Only checks fields that are reliably present in WebSocket payloads
 */
function enforcementMatchesParams(
  enforcement: Partial<EnforcementSummary>,
  params: EnforcementQueryParams | undefined,
): boolean {
  if (!params) return true;

  // Check status filter
  if (params.status && enforcement.status !== params.status) {
    return false;
  }

  // Check event filter
  if (params.event !== undefined && enforcement.event !== params.event) {
    return false;
  }

  // Check rule filter
  if (params.rule !== undefined && enforcement.rule !== params.rule) {
    return false;
  }

  // Check trigger_ref filter (always present)
  if (params.triggerRef && enforcement.trigger_ref !== params.triggerRef) {
    return false;
  }

  // Note: rule_ref is NOT checked here for new enforcements because it may not be
  // present in WebSocket payloads. For this filter, we only update existing
  // enforcements, never add new ones.

  return true;
}

/**
 * Check if query params include filters not present in WebSocket payloads
 */
function hasUnsupportedFilters(
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  _params: EnforcementQueryParams | undefined,
): boolean {
  // Currently all enforcement filters are supported in WebSocket payloads
  return false;
}

/**
 * Hook to subscribe to real-time enforcement updates via WebSocket.
 *
 * Automatically reconnects on connection loss and updates React Query cache
 * when enforcement updates are received.
 *
 * @example
 * ```tsx
 * // Listen to all enforcement updates
 * useEnforcementStream();
 *
 * // Listen to updates for a specific enforcement
 * useEnforcementStream({ enforcementId: 123 });
 * ```
 */
export function useEnforcementStream(
  options: UseEnforcementStreamOptions = {},
) {
  const {
    enforcementId,
    enabled = true,
    paused = false,
    maxListItems = 100,
  } = options;
  const queryClient = useQueryClient();

  const handleNotification = useCallback(
    (raw: Notification) => {
      if (paused) {
        return;
      }

      const notification = raw as unknown as EnforcementNotification;
      // Filter by enforcement ID if specified
      if (enforcementId && notification.entity_id !== enforcementId) {
        return;
      }

      // Extract enforcement data from notification payload (flat structure)
      const enforcementData =
        notification.payload as Partial<EnforcementSummary>;

      // Update specific enforcement query if it exists
      queryClient.setQueryData(
        ["enforcements", notification.entity_id],
        (old: EnforcementDetailCache | undefined) => {
          if (!old) return old;
          return {
            ...old,
            data: {
              ...old.data,
              ...enforcementData,
            },
          };
        },
      );

      // Update enforcement list queries by modifying existing data
      // We need to iterate manually to access query keys for filtering
      const queries = queryClient
        .getQueriesData<EnforcementListCache>({
          queryKey: ["enforcements"],
          exact: false,
        })
        .filter(([, data]) => data && Array.isArray(data?.items));

      queries.forEach(([queryKey, oldData]) => {
        // Extract query params from the query key (format: ["enforcements", params])
        const queryParams = queryKey[1] as EnforcementQueryParams | undefined;

        const old = oldData as EnforcementListCache;

        // Check if enforcement already exists in the list
        const existingIndex = old.items.findIndex(
          (enf) => enf.id === notification.entity_id,
        );

        let updatedItems: EnforcementSummary[];
        if (existingIndex >= 0) {
          // Always update existing enforcement in the list
          updatedItems = [...old.items];
          updatedItems[existingIndex] = {
            ...updatedItems[existingIndex],
            ...enforcementData,
          };

          // Note: We don't remove enforcements from cache based on filters.
          // The cache represents what the API query returned.
          // Client-side filtering (in the page component) handles what's displayed.
        } else {
          // For new enforcements, be conservative with filters we can't verify
          if (hasUnsupportedFilters(queryParams)) {
            // Don't add new enforcement when using filters we can't verify
            return;
          }

          // Only add new enforcement if it matches the query parameters
          if (enforcementMatchesParams(enforcementData, queryParams)) {
            // Add to beginning and cap retained items to prevent performance issues
            updatedItems = [
              enforcementData as EnforcementSummary,
              ...old.items,
            ].slice(0, maxListItems);
          } else {
            // Don't modify the list if the new enforcement doesn't match the query
            return;
          }
        }

        const totalItemsDelta = existingIndex >= 0 ? 0 : 1;
        const page = old.pagination?.page ?? 1;
        const pageSize = old.pagination?.page_size ?? maxListItems;
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
          } else if (totalItemsDelta > 0 && old.items.length >= pageSize) {
            nextPagination.has_next = true;
          }
        }

        // Update the query with the new data
        queryClient.setQueryData(queryKey, {
          ...old,
          items: updatedItems,
          pagination: nextPagination,
        });
      });

      // Also update related queries (rules and events enforcements)
      if (enforcementData.rule) {
        queryClient.invalidateQueries({
          queryKey: ["rules", enforcementData.rule, "enforcements"],
        });
      }
      if (enforcementData.event) {
        queryClient.invalidateQueries({
          queryKey: ["events", enforcementData.event, "enforcements"],
        });
      }
    },
    [enforcementId, maxListItems, paused, queryClient],
  );

  const { connected } = useEntityNotifications(
    "enforcement",
    handleNotification,
    enabled && !paused,
  );

  return {
    isConnected: connected,
    error: null,
  };
}
