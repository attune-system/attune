import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { ActionsService } from "@/api";
import type { CreateActionRequest, UpdateActionRequest } from "@/api";

interface ActionsQueryParams {
  page?: number;
  pageSize?: number;
  packRef?: string;
  referencingPackRef?: string;
}

// Fetch all actions with pagination
export function useActions(params?: ActionsQueryParams) {
  return useQuery({
    queryKey: ["actions", params],
    queryFn: async () => {
      const response = await ActionsService.listActions({
        page: params?.page || 1,
        pageSize: params?.pageSize || 50,
        referencingPackRef: params?.referencingPackRef,
      });
      return response;
    },
    staleTime: 30000, // 30 seconds
  });
}

// Fetch single action by ref
export function useAction(ref: string) {
  return useQuery({
    queryKey: ["actions", ref],
    queryFn: async () => {
      const response = await ActionsService.getAction({ ref });
      return response;
    },
    enabled: !!ref,
    staleTime: 30000,
  });
}

// Fetch actions by pack
export function usePackActions(packRef: string) {
  return useQuery({
    queryKey: ["packs", packRef, "actions"],
    queryFn: async () => {
      const response = await ActionsService.listActionsByPack({ packRef });
      return response.items;
    },
    enabled: !!packRef,
    staleTime: 30000,
  });
}

// Create a new action
export function useCreateAction() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreateActionRequest) => {
      const response = await ActionsService.createAction({ requestBody: data });
      return response;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["actions"] });
    },
  });
}

// Update existing action
export function useUpdateAction() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      data,
    }: {
      ref: string;
      data: UpdateActionRequest;
    }) => {
      const response = await ActionsService.updateAction({
        ref,
        requestBody: data,
      });
      return response;
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["actions"] });
      queryClient.invalidateQueries({ queryKey: ["actions", variables.ref] });
    },
  });
}

// Delete action
export function useDeleteAction() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      await ActionsService.deleteAction({ ref });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["actions"] });
    },
  });
}
