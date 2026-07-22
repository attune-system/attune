import { useCallback } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CachesService,
  type CacheEntryUpload,
  type CacheNamespaceFreshness,
  type CacheOwnerParams,
  type CreateCacheGenerationRequest,
  type CreateCacheNamespaceRequest,
  type PromoteCacheGenerationRequest,
  type SealCacheGenerationRequest,
  type UpdateCacheNamespaceRequest,
} from "@/api/cache";

/**
 * Query key factory for the Data Caches feature.
 *
 * Entry scans have separate key branches for the mutable current-generation
 * pointer and immutable generation-pinned snapshots. Promotion/restart can
 * therefore reset current resolution without disturbing traversal pages.
 */
export const cacheKeys = {
  all: ["caches"] as const,
  lists: () => [...cacheKeys.all, "list"] as const,
  // The API has no cross-owner "list everything" mode (owner_type is
  // mandatory on every route), so the list key is always owner-scoped.
  list: (
    owner: CacheOwnerParams,
    shape: {
      namespace?: string;
      freshness?: CacheNamespaceFreshness;
      limit?: number;
      cursor?: string;
    } = {},
  ) =>
    [
      ...cacheKeys.lists(),
      owner.ownerType,
      owner.ownerRef ?? null,
      shape.namespace ?? null,
      shape.freshness ?? null,
      shape.limit ?? null,
      shape.cursor ?? null,
    ] as const,
  namespaces: () => [...cacheKeys.all, "namespace"] as const,
  namespace: (owner: CacheOwnerParams, namespace: string) =>
    [
      ...cacheKeys.namespaces(),
      owner.ownerType,
      owner.ownerRef ?? null,
      namespace,
    ] as const,
  generations: (owner: CacheOwnerParams, namespace: string) =>
    [...cacheKeys.namespace(owner, namespace), "generations"] as const,
  generationList: (
    owner: CacheOwnerParams,
    namespace: string,
    shape: { limit?: number; cursor?: string } = {},
  ) =>
    [
      ...cacheKeys.generations(owner, namespace),
      "list",
      shape.limit ?? null,
      shape.cursor ?? null,
    ] as const,
  generation: (
    owner: CacheOwnerParams,
    namespace: string,
    generationId: number,
  ) => [...cacheKeys.generations(owner, namespace), generationId] as const,
  entryScans: (owner: CacheOwnerParams, namespace: string) =>
    [...cacheKeys.namespace(owner, namespace), "entries", "scan"] as const,
  currentEntryScans: (owner: CacheOwnerParams, namespace: string) =>
    [...cacheKeys.entryScans(owner, namespace), "current"] as const,
  generationEntryScans: (
    owner: CacheOwnerParams,
    namespace: string,
    generationId: number,
  ) =>
    [
      ...cacheKeys.entryScans(owner, namespace),
      "generation",
      generationId,
    ] as const,
  entryScan: (
    owner: CacheOwnerParams,
    namespace: string,
    shape: {
      generationId?: number;
      cursor?: string;
      limit?: number;
      requireFresh?: boolean;
    },
  ) =>
    [
      ...(shape.generationId === undefined
        ? cacheKeys.currentEntryScans(owner, namespace)
        : cacheKeys.generationEntryScans(owner, namespace, shape.generationId)),
      shape.cursor ?? null,
      shape.limit ?? null,
      shape.requireFresh ?? false,
    ] as const,
};

// ── Namespaces ──────────────────────────────────────────────────────────────
//
// `GET /cache/namespaces` requires an owner scope and applies metadata filters
// before an opaque keyset cursor, so each query key includes the full page
// shape.
export interface CacheNamespaceListShape {
  namespace?: string;
  freshness?: CacheNamespaceFreshness;
  limit?: number;
  cursor?: string;
}

export function useCacheNamespaces(
  owner: CacheOwnerParams | undefined,
  shape: CacheNamespaceListShape = {},
) {
  return useQuery({
    queryKey: cacheKeys.list(owner ?? { ownerType: undefined as never }, shape),
    queryFn: () => CachesService.listNamespaces({ owner: owner!, ...shape }),
    enabled: Boolean(owner?.ownerType),
    staleTime: 15000,
  });
}

export function useCacheNamespace(
  owner: CacheOwnerParams | undefined,
  namespace: string | undefined,
) {
  return useQuery({
    queryKey: cacheKeys.namespace(
      owner ?? { ownerType: undefined as never },
      namespace ?? "",
    ),
    queryFn: () =>
      CachesService.getNamespace({ owner: owner!, namespace: namespace! }),
    enabled: Boolean(owner?.ownerType && namespace),
    staleTime: 15000,
  });
}

export function useCreateCacheNamespace() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: CreateCacheNamespaceRequest) =>
      CachesService.createNamespace({ requestBody: data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: cacheKeys.lists() });
    },
  });
}

export function useUpdateCacheNamespacePolicy(
  owner: CacheOwnerParams,
  namespace: string,
) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: UpdateCacheNamespaceRequest) =>
      CachesService.updateNamespacePolicy({ namespace, requestBody: data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: cacheKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: cacheKeys.namespace(owner, namespace),
      });
    },
  });
}

export function useDeleteCacheNamespace() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      owner,
      namespace,
    }: {
      owner: CacheOwnerParams;
      namespace: string;
    }) => CachesService.deleteNamespace({ owner, namespace }),
    onSuccess: (_response, variables) => {
      queryClient.invalidateQueries({ queryKey: cacheKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: cacheKeys.namespace(variables.owner, variables.namespace),
      });
    },
  });
}

// ── Generations ───────────────────────────────────────────────────────────

export function useCacheGenerations(
  owner: CacheOwnerParams | undefined,
  namespace: string | undefined,
  shape: { limit?: number; cursor?: string } = {},
) {
  return useQuery({
    queryKey: cacheKeys.generationList(
      owner ?? { ownerType: undefined as never },
      namespace ?? "",
      shape,
    ),
    queryFn: () =>
      CachesService.listGenerations({
        owner: owner!,
        namespace: namespace!,
        ...shape,
      }),
    enabled: Boolean(owner?.ownerType && namespace),
    staleTime: 10000,
  });
}

export function useCacheGeneration(
  owner: CacheOwnerParams | undefined,
  namespace: string | undefined,
  generationId: number | undefined,
) {
  return useQuery({
    queryKey: cacheKeys.generation(
      owner ?? { ownerType: undefined as never },
      namespace ?? "",
      generationId ?? 0,
    ),
    queryFn: () =>
      CachesService.getGeneration({
        owner: owner!,
        namespace: namespace!,
        generationId: generationId!,
      }),
    enabled: Boolean(owner?.ownerType && namespace && generationId),
    staleTime: 10000,
  });
}

// ── Entries (deliberate, bounded reads only) ────────────────────────────
//
// Point lookup and bounded multi-lookup are modeled as mutations rather than
// queries: they must only run when a caller explicitly asks for a specific
// record (or bounded set of records), never ambiently on render/navigation,
// and their results intentionally are not cached/reused across renders since
// they can contain sensitive business data.

export function useCacheEntryLookup(
  owner: CacheOwnerParams,
  namespace: string,
) {
  return useMutation({
    mutationFn: (externalId: string) =>
      CachesService.getEntry({ owner, namespace, externalId }),
  });
}

export function useCacheEntriesGetMany(
  owner: CacheOwnerParams,
  namespace: string,
) {
  return useMutation({
    mutationFn: (externalIds: string[]) =>
      CachesService.getEntries({ owner, namespace, externalIds }),
  });
}

export interface CacheEntryScanShape {
  generationId?: number;
  cursor?: string;
  limit?: number;
  requireFresh?: boolean;
  /** Caller must opt in explicitly; browsing never starts on mount by accident. */
  enabled?: boolean;
}

export function useCacheEntryScan(
  owner: CacheOwnerParams | undefined,
  namespace: string | undefined,
  shape: CacheEntryScanShape,
) {
  return useQuery({
    queryKey: cacheKeys.entryScan(
      owner ?? { ownerType: undefined as never },
      namespace ?? "",
      shape,
    ),
    queryFn: () =>
      CachesService.scanEntries({
        owner: owner!,
        namespace: namespace!,
        generationId: shape.generationId,
        cursor: shape.cursor,
        limit: shape.limit,
        requireFresh: shape.requireFresh,
      }),
    enabled: Boolean(owner?.ownerType && namespace && shape.enabled),
    staleTime: 10000,
    // Cursor/generation-pinned pages are immutable snapshots; never
    // auto-refetch a page out from under an in-progress browse.
    refetchOnWindowFocus: false,
  });
}

export function useResetCurrentCacheEntryScans(
  owner: CacheOwnerParams,
  namespace: string,
) {
  const queryClient = useQueryClient();

  return useCallback(
    () =>
      queryClient.resetQueries({
        queryKey: cacheKeys.currentEntryScans(owner, namespace),
      }),
    [namespace, owner, queryClient],
  );
}

// ── Refresh lifecycle ────────────────────────────────────────────────────
//
// Unlike the read hooks above, these all take `owner` as a hook param *and*
// use it to fill in the request body's required `owner_type`/`owner_ref`
// fields, so call sites only need to supply the operation-specific fields.

export function useBeginCacheRefresh(
  owner: CacheOwnerParams,
  namespace: string,
) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (
      data: Omit<CreateCacheGenerationRequest, "owner_type" | "owner_ref">,
    ) =>
      CachesService.beginRefresh({
        namespace,
        requestBody: {
          ...data,
          owner_type: owner.ownerType,
          owner_ref: owner.ownerRef || undefined,
        },
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: cacheKeys.generations(owner, namespace),
      });
    },
  });
}

export function useUploadCacheChunk(
  owner: CacheOwnerParams,
  namespace: string,
) {
  return useMutation({
    mutationFn: ({
      generationId,
      chunkIndex,
      entries,
    }: {
      generationId: number;
      chunkIndex: number;
      entries: CacheEntryUpload[];
    }) =>
      CachesService.uploadRefreshChunk({
        owner,
        namespace,
        generationId,
        chunkIndex,
        entries,
      }),
  });
}

export function useSealCacheGeneration(
  owner: CacheOwnerParams,
  namespace: string,
) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      generationId,
      data,
    }: {
      generationId: number;
      data: Omit<SealCacheGenerationRequest, "owner_type" | "owner_ref">;
    }) =>
      CachesService.sealGeneration({
        namespace,
        generationId,
        requestBody: {
          ...data,
          owner_type: owner.ownerType,
          owner_ref: owner.ownerRef || undefined,
        },
      }),
    onSuccess: (_response, variables) => {
      queryClient.invalidateQueries({
        queryKey: cacheKeys.generations(owner, namespace),
      });
      queryClient.invalidateQueries({
        queryKey: cacheKeys.generation(
          owner,
          namespace,
          variables.generationId,
        ),
      });
    },
  });
}

export function usePromoteCacheGeneration(
  owner: CacheOwnerParams,
  namespace: string,
) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      generationId,
      data,
    }: {
      generationId: number;
      data: Omit<PromoteCacheGenerationRequest, "owner_type" | "owner_ref">;
    }) =>
      CachesService.promoteGeneration({
        namespace,
        generationId,
        requestBody: {
          ...data,
          owner_type: owner.ownerType,
          owner_ref: owner.ownerRef || undefined,
        },
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: cacheKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: cacheKeys.namespace(owner, namespace),
        exact: true,
      });
      queryClient.invalidateQueries({
        queryKey: cacheKeys.generations(owner, namespace),
      });
      queryClient.resetQueries({
        queryKey: cacheKeys.currentEntryScans(owner, namespace),
      });
    },
  });
}

export function useAbandonCacheGeneration(
  owner: CacheOwnerParams,
  namespace: string,
) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (generationId: number) =>
      CachesService.abandonGeneration({ owner, namespace, generationId }),
    onSuccess: (_response, generationId) => {
      queryClient.invalidateQueries({
        queryKey: cacheKeys.generations(owner, namespace),
      });
      queryClient.invalidateQueries({
        queryKey: cacheKeys.generation(owner, namespace, generationId),
      });
    },
  });
}
