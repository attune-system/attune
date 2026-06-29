import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { WorkersService } from "@/api/workers";
import type {
  WorkerHealthState,
  WorkerRole,
  WorkerStatus,
} from "@/api/workers";

export function useWorkers(params?: {
  page?: number;
  pageSize?: number;
  role?: WorkerRole;
  status?: WorkerStatus;
  cordoned?: boolean;
  healthState?: WorkerHealthState;
  enabled?: boolean;
}) {
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
