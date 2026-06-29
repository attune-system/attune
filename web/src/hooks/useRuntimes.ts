import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  RuntimesService,
  type CreateRuntimeRequest,
  type UpdateRuntimeRequest,
} from "@/api";

export function useRuntimes(options?: { enabled?: boolean }) {
  return useQuery({
    queryKey: ["runtimes"],
    queryFn: async () =>
      RuntimesService.listRuntimes({ page: 1, pageSize: 100 }),
    enabled: options?.enabled ?? true,
    staleTime: 30000,
  });
}

export function useRuntime(ref: string, options?: { enabled?: boolean }) {
  return useQuery({
    queryKey: ["runtimes", ref],
    queryFn: async () => RuntimesService.getRuntime({ ref }),
    enabled: (options?.enabled ?? true) && !!ref && ref !== "new",
    staleTime: 30000,
  });
}

export function useCreateRuntime() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreateRuntimeRequest) =>
      RuntimesService.createRuntime({ requestBody: data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["runtimes"] });
    },
  });
}

export function useUpdateRuntime() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      data,
    }: {
      ref: string;
      data: UpdateRuntimeRequest;
    }) => RuntimesService.updateRuntime({ ref, requestBody: data }),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["runtimes"] });
      queryClient.invalidateQueries({ queryKey: ["runtimes", variables.ref] });
    },
  });
}

export function useDeleteRuntime() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => RuntimesService.deleteRuntime({ ref }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["runtimes"] });
    },
  });
}
