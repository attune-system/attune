import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { TriggersService } from "@/api";
import type { CreateTriggerRequest, UpdateTriggerRequest } from "@/api";

interface TriggersQueryParams {
  page?: number;
  pageSize?: number;
  packRef?: string;
  enabled?: boolean;
  referencingPackRef?: string;
}

// Fetch all triggers with pagination
export function useTriggers(params?: TriggersQueryParams) {
  return useQuery({
    queryKey: ["triggers", params],
    queryFn: async () => {
      if (params?.packRef) {
        return await TriggersService.listTriggersByPack({
          packRef: params.packRef,
          page: params?.page || 1,
          pageSize: params?.pageSize || 50,
        });
      }
      return await TriggersService.listTriggers({
        page: params?.page || 1,
        pageSize: params?.pageSize || 50,
        referencingPackRef: params?.referencingPackRef,
      });
    },
    staleTime: 30000, // 30 seconds
  });
}

// Fetch enabled triggers only
export function useEnabledTriggers(
  params?: Omit<TriggersQueryParams, "enabled">,
) {
  return useTriggers({ ...params, enabled: true });
}

// Fetch single trigger by reference
export function useTrigger(ref: string, referencingPackRef?: string) {
  return useQuery({
    queryKey: ["triggers", ref, referencingPackRef],
    queryFn: async () => {
      return await TriggersService.getTrigger({ ref, referencingPackRef });
    },
    enabled: !!ref,
    staleTime: 30000,
  });
}

// Fetch triggers by pack
export function usePackTriggers(packRef: string) {
  return useQuery({
    queryKey: ["packs", packRef, "triggers"],
    queryFn: async () => {
      return await TriggersService.listTriggersByPack({
        packRef,
        page: 1,
        pageSize: 100,
      });
    },
    enabled: !!packRef,
    staleTime: 30000,
  });
}

// Create a new trigger
export function useCreateTrigger() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreateTriggerRequest) => {
      return await TriggersService.createTrigger({ requestBody: data });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["triggers"] });
    },
  });
}

// Update existing trigger
export function useUpdateTrigger() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      data,
    }: {
      ref: string;
      data: UpdateTriggerRequest;
    }) => {
      return await TriggersService.updateTrigger({ ref, requestBody: data });
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["triggers"] });
      queryClient.invalidateQueries({ queryKey: ["triggers", variables.ref] });
    },
  });
}

// Delete trigger
export function useDeleteTrigger() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      await TriggersService.deleteTrigger({ ref });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["triggers"] });
    },
  });
}

// Enable trigger
export function useEnableTrigger() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      return await TriggersService.enableTrigger({ ref });
    },
    onSuccess: (_, ref) => {
      queryClient.invalidateQueries({ queryKey: ["triggers"] });
      queryClient.invalidateQueries({ queryKey: ["triggers", ref] });
    },
  });
}

// Disable trigger
export function useDisableTrigger() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      return await TriggersService.disableTrigger({ ref });
    },
    onSuccess: (_, ref) => {
      queryClient.invalidateQueries({ queryKey: ["triggers"] });
      queryClient.invalidateQueries({ queryKey: ["triggers", ref] });
    },
  });
}
