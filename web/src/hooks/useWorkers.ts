import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { WorkersService } from "@/api/workers";
import { OpenAPI } from "@/api/core/OpenAPI";
import { request as apiRequest } from "@/api/core/request";
import type {
  PaginatedResponseWorkerSummary,
  WorkerHealthState,
  WorkerRole,
  WorkerStatus,
} from "@/api/workers";

interface WorkersQueryParams {
  page?: number;
  pageSize?: number;
  role?: WorkerRole;
  status?: WorkerStatus;
  cordoned?: boolean;
  healthState?: WorkerHealthState;
  enabled?: boolean;
  query?: string;
}

export function useWorkers(params?: WorkersQueryParams) {
  return useQuery({
    queryKey: ["workers", params],
    queryFn: async () =>
      WorkersService.listWorkers({
        page: params?.page || 1,
        pageSize: params?.pageSize || 100,
        role: params?.role,
        status: params?.status,
        cordoned: params?.cordoned,
        healthState: params?.healthState,
      }),
    enabled: params?.enabled ?? true,
    staleTime: 30000,
  });
}

export function useInfiniteWorkers(
  params?: Omit<WorkersQueryParams, "page" | "pageSize">,
) {
  return useInfiniteQuery({
    queryKey: ["workers", "infinite", params],
    initialPageParam: 1,
    queryFn: ({ pageParam }) =>
      apiRequest<PaginatedResponseWorkerSummary>(OpenAPI, {
        method: "GET",
        url: "/api/v1/workers",
        query: {
          page: pageParam,
          page_size: 100,
          role: params?.role,
          status: params?.status,
          cordoned: params?.cordoned,
          health_state: params?.healthState,
          q: params?.query || undefined,
        },
      }),
    getNextPageParam: (lastPage) =>
      lastPage.pagination.has_next ? lastPage.pagination.page + 1 : undefined,
    enabled: params?.enabled ?? true,
    staleTime: 30000,
  });
}

export function useWorker(id: number | null | undefined) {
  return useQuery({
    queryKey: ["workers", id],
    queryFn: async () => WorkersService.getWorker({ id: id! }),
    enabled: !!id,
    staleTime: 30000,
  });
}

export function useCordonWorker() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: WorkersService.cordonWorker,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["workers"] });
    },
  });
}

export function useUncordonWorker() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: WorkersService.uncordonWorker,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["workers"] });
    },
  });
}
