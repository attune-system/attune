import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { PacksService } from "@/api";
import type {
  CreatePackRequest,
  CreatePackRegistryIndexRequest,
  UpdatePackRegistryIndexRequest,
  UpdatePackRequest,
} from "@/api";

interface PacksQueryParams {
  page?: number;
  pageSize?: number;
}

// Fetch one page of packs.
export function usePacks(params?: PacksQueryParams) {
  return useQuery({
    queryKey: ["packs", params],
    queryFn: async () => {
      const response = await PacksService.listPacks({
        page: params?.page || 1,
        pageSize: params?.pageSize || 50,
      });
      return response;
    },
    staleTime: 30000, // 30 seconds
  });
}

// Fetch pack pages only as the catalog scrolls.
export function useInfinitePacks(query?: string) {
  return useInfiniteQuery({
    queryKey: ["packs", "infinite", query],
    initialPageParam: 1,
    queryFn: ({ pageParam }) =>
      PacksService.listPacks({
        page: pageParam,
        pageSize: 50,
        q: query || undefined,
      }),
    getNextPageParam: (lastPage) =>
      lastPage.pagination.has_next ? lastPage.pagination.page + 1 : undefined,
    staleTime: 30000, // 30 seconds
  });
}

// Fetch single pack by ref
export function usePack(ref: string) {
  return useQuery({
    queryKey: ["packs", ref],
    queryFn: async () => {
      const response = await PacksService.getPack({ ref });
      return response;
    },
    enabled: !!ref,
    staleTime: 30000,
  });
}

// Create a new pack
export function useCreatePack() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (data: CreatePackRequest) => {
      const response = await PacksService.createPack({ requestBody: data });
      return response;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["packs"] });
    },
  });
}

// Update existing pack
export function useUpdatePack() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      data,
    }: {
      ref: string;
      data: UpdatePackRequest;
    }) => {
      const response = await PacksService.updatePack({
        ref,
        requestBody: data,
      });
      return response;
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["packs"] });
      queryClient.invalidateQueries({ queryKey: ["packs", variables.ref] });
    },
  });
}

// Delete pack
export function useDeletePack() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => {
      await PacksService.deletePack({ ref });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["packs"] });
    },
  });
}

export function usePackIndices() {
  return useQuery({
    queryKey: ["pack-indices"],
    queryFn: () => PacksService.listPackIndices(),
    staleTime: 30000,
  });
}

export function useCreatePackIndex() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: CreatePackRegistryIndexRequest) =>
      PacksService.createPackIndex({ requestBody: data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["pack-indices"] });
      queryClient.invalidateQueries({ queryKey: ["indexed-packs"] });
    },
  });
}

export function useUpdatePackIndex() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      data,
    }: {
      id: number;
      data: UpdatePackRegistryIndexRequest;
    }) => PacksService.updatePackIndex({ id, requestBody: data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["pack-indices"] });
      queryClient.invalidateQueries({ queryKey: ["indexed-packs"] });
    },
  });
}

export function useDeletePackIndex() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => PacksService.deletePackIndex({ id }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["pack-indices"] });
      queryClient.invalidateQueries({ queryKey: ["indexed-packs"] });
    },
  });
}

export function useIndexedPacks(query?: string) {
  return useQuery({
    queryKey: ["indexed-packs", query],
    queryFn: () => PacksService.browseIndexedPacks({ q: query || undefined }),
    staleTime: 30000,
  });
}

export function usePackReleases(ref: string) {
  return useQuery({
    queryKey: ["packs", ref, "releases"],
    queryFn: () => PacksService.getPackReleases({ ref }),
    enabled: !!ref,
    staleTime: 30000,
  });
}

export function useRetiredPackComponents(ref: string, enabled = true) {
  return useQuery({
    queryKey: ["packs", ref, "retired-components"],
    queryFn: () => PacksService.getRetiredPackComponents({ ref }),
    enabled: !!ref && enabled,
    staleTime: 30000,
  });
}

export function usePlatformCatalog() {
  return useQuery({
    queryKey: ["platform-catalog"],
    queryFn: () => PacksService.getPlatformCatalog(),
    staleTime: 60000,
  });
}
