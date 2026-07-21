import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { ActionsService } from "@/api";
import { OpenAPI } from "@/api/core/OpenAPI";
import { request as apiRequest } from "@/api/core/request";
import type {
  CreateActionRequest,
  PaginatedResponse_ActionSummary,
  UpdateActionRequest,
} from "@/api";

interface ActionsQueryParams {
  page?: number;
  pageSize?: number;
  packRef?: string;
  referencingPackRef?: string;
}

interface InfiniteActionsQueryParams extends Omit<
  ActionsQueryParams,
  "page" | "pageSize"
> {
  query?: string;
}

// Fetch one page of actions.
export function useActions(params?: ActionsQueryParams) {
  return useQuery({
    queryKey: ["actions", params],
    queryFn: async () => {
      if (params?.packRef) {
        return await ActionsService.listActionsByPack({
          packRef: params.packRef,
          page: params.page || 1,
          pageSize: params.pageSize || 50,
        });
      }
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

// Fetch action pages only as the catalog scrolls.
export function useInfiniteActions(params?: InfiniteActionsQueryParams) {
  return useInfiniteQuery({
    queryKey: ["actions", "infinite", params],
    initialPageParam: 1,
    queryFn: ({ pageParam }) => {
      return apiRequest<PaginatedResponse_ActionSummary>(OpenAPI, {
        method: "GET",
        url: params?.packRef
          ? "/api/v1/packs/{pack_ref}/actions"
          : "/api/v1/actions",
        path: params?.packRef ? { pack_ref: params.packRef } : undefined,
        query: {
          page: pageParam,
          page_size: 50,
          q: params?.query || undefined,
          referencing_pack_ref: params?.referencingPackRef,
        },
      });
    },
    getNextPageParam: (lastPage) =>
      lastPage.pagination.has_next ? lastPage.pagination.page + 1 : undefined,
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
    queryFn: () =>
      ActionsService.listActionsByPack({
        packRef,
        page: 1,
        pageSize: 50,
      }),
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
