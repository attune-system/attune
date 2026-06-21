import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from "@tanstack/react-query";
import { ExecutionsService } from "@/api";
import type { ExecutionStatus } from "@/api";
import { OpenAPI } from "@/api/core/OpenAPI";
import { request as __request } from "@/api/core/request";
import type { ExecutionResponse } from "@/api";

interface ExecutionsQueryParams {
  page?: number;
  pageSize?: number;
  status?: ExecutionStatus;
  actionRef?: string;
  ruleRef?: string;
  triggerRef?: string;
  traceTag?: string;
  executor?: number;
  topLevelOnly?: boolean;
  includeTotal?: boolean;
}

function isExecutionActive(status: string | undefined): boolean {
  return (
    status === "requested" ||
    status === "scheduling" ||
    status === "scheduled" ||
    status === "running" ||
    status === "canceling"
  );
}

export function useExecutions(params?: ExecutionsQueryParams) {
  // Check if any filters are applied
  const hasFilters =
    params?.status ||
    params?.actionRef ||
    params?.ruleRef ||
    params?.triggerRef ||
    params?.traceTag ||
    params?.executor ||
    params?.topLevelOnly;

  return useQuery({
    queryKey: ["executions", params],
    queryFn: async () => {
      const response = await ExecutionsService.listExecutions({
        page: params?.page,
        perPage: params?.pageSize,
        status: params?.status,
        actionRef: params?.actionRef,
        ruleRef: params?.ruleRef,
        triggerRef: params?.triggerRef,
        traceTag: params?.traceTag,
        executor: params?.executor,
        topLevelOnly: params?.topLevelOnly,
        includeTotal: params?.includeTotal,
      });
      return response;
    },
    // Use shorter staleTime when filters are active to ensure fresh results
    // Use longer staleTime for unfiltered list since SSE handles real-time updates
    staleTime: hasFilters ? 5000 : 30000,
    // Refetch in background when filters change to get latest data
    refetchOnMount: hasFilters ? "always" : true,
    // Keep previous results visible while new data loads (prevents flash of empty state)
    placeholderData: keepPreviousData,
  });
}

export function useExecution(id: number) {
  return useQuery({
    queryKey: ["executions", id],
    queryFn: async () => {
      const response = await ExecutionsService.getExecution({ id });
      return response;
    },
    enabled: !!id,
    staleTime: 30000,
    refetchInterval: (query) =>
      isExecutionActive(query.state.data?.data?.status) ? 3000 : false,
  });
}

/**
 * Fetch child executions (workflow tasks) for a given parent execution ID.
 *
 * Enabled only when `parentId` is provided. Polls every 5 seconds while any
 * child execution is still in a running/pending state so the UI stays current.
 */
/**
 * Request a manual execution of an action (or workflow).
 *
 * Calls POST /api/v1/executions/execute and returns the created execution,
 * including its `id` which callers can use to navigate to the detail page.
 */
export function useRequestExecution() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      actionRef,
      parameters,
    }: {
      actionRef: string;
      parameters?: Record<string, unknown>;
    }) => {
      const response = await __request(OpenAPI, {
        method: "POST",
        url: "/api/v1/executions/execute",
        body: {
          action_ref: actionRef,
          parameters: parameters ?? null,
        },
        mediaType: "application/json",
      });
      return response as {
        data: { id: number; status: string; action_ref: string };
      };
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["executions"] });
    },
  });
}

/**
 * Cancel a running or pending execution.
 *
 * Calls POST /api/v1/executions/{id}/cancel. For workflow executions this
 * cascades to all incomplete child task executions on the server side.
 */
export function useCancelExecution() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (executionId: number) => {
      const response = await __request(OpenAPI, {
        method: "POST",
        url: "/api/v1/executions/{id}/cancel",
        path: { id: executionId },
        mediaType: "application/json",
      });
      return response as { data: ExecutionResponse };
    },
    onSuccess: (_data, executionId) => {
      // Invalidate the specific execution and the list
      queryClient.invalidateQueries({ queryKey: ["executions", executionId] });
      queryClient.invalidateQueries({ queryKey: ["executions"] });
    },
  });
}

/**
 * Republish the scheduler message for a stuck requested execution.
 */
export function useRescheduleExecution() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (executionId: number) => {
      const response = await __request(OpenAPI, {
        method: "POST",
        url: "/api/v1/executions/{id}/reschedule",
        path: { id: executionId },
        mediaType: "application/json",
      });
      return response as {
        data: {
          message: string;
          attempt_count: number;
          last_attempt_at: string;
          execution: ExecutionResponse;
        };
      };
    },
    onSuccess: (_data, executionId) => {
      queryClient.invalidateQueries({ queryKey: ["executions", executionId] });
      queryClient.invalidateQueries({ queryKey: ["executions"] });
    },
  });
}

export function useChildExecutions(
  parentId: number | undefined,
  options: { includeDescendants?: boolean } = {},
) {
  const { includeDescendants = false } = options;
  return useQuery({
    queryKey: [
      "executions",
      { parent: parentId, descendants: includeDescendants },
    ],
    queryFn: async () => {
      const fetchAllChildren = async (pid: number) => {
        const first = await ExecutionsService.listExecutions({
          parent: pid,
          includeTotal: true,
          perPage: 100,
          page: 1,
        });
        const totalPages = first.pagination.total_pages ?? 1;
        if (totalPages > 1) {
          const remaining = await Promise.all(
            Array.from({ length: totalPages - 1 }, (_, i) =>
              ExecutionsService.listExecutions({
                parent: pid,
                includeTotal: true,
                perPage: 100,
                page: i + 2,
              }),
            ),
          );
          for (const page of remaining) {
            first.items.push(...page.items);
          }
        }
        return first;
      };

      const root = await fetchAllChildren(parentId!);

      if (includeDescendants) {
        // BFS over descendants. Bounded by the actual tree.
        const visited = new Set<number>([parentId!]);
        const queue: number[] = root.items
          .map((e) => e.id)
          .filter((id) => !visited.has(id));
        for (const id of queue) visited.add(id);

        while (queue.length > 0) {
          const layer = queue.splice(0, queue.length);
          const results = await Promise.all(
            layer.map((id) => fetchAllChildren(id)),
          );
          for (const r of results) {
            for (const e of r.items) {
              if (!visited.has(e.id)) {
                visited.add(e.id);
                root.items.push(e);
                queue.push(e.id);
              }
            }
          }
        }
      }

      root.pagination.total_pages = 1;
      root.pagination.page_size = root.items.length;
      root.pagination.has_next = false;
      root.pagination.has_previous = false;
      return root;
    },
    enabled: !!parentId,
    staleTime: 5000,
    // Re-fetch periodically so in-progress tasks update
    refetchInterval: (query) => {
      const data = query.state.data;
      if (!data) return false;
      const hasActive = data.items.some((e) => isExecutionActive(e.status));
      return hasActive ? 5000 : false;
    },
  });
}
