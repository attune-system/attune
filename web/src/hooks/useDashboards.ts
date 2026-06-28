import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  cloneDashboard,
  createDashboard,
  deleteDashboard,
  fetchDashboardData,
  fetchDashboardList,
  fetchDashboardMetadata,
  fetchDashboardSourceCatalog,
  fetchDashboardSpec,
  previewDashboard,
  updateDashboard,
} from "@/lib/dashboard-client";
import type {
  DashboardAuthoringDocument,
  DashboardCloneRequest,
  DashboardCreateRequest,
  DashboardDataRequest,
  DashboardFilterValue,
  DashboardListItem,
  DashboardMetadataResponse,
  DashboardPreviewRequest,
  DashboardSourceCatalogResponse,
  DashboardSpec,
  DashboardUpdateRequest,
} from "@/types/dashboard";

function dedupe(values: string[] | undefined): string[] | undefined {
  if (!values?.length) return undefined;
  const seen = new Set<string>();
  const out: string[] = [];
  for (const value of values) {
    if (!seen.has(value)) {
      seen.add(value);
      out.push(value);
    }
  }
  return out;
}

function canonicalizeSourceIds(
  sourceIds: string[] | undefined,
): string[] | undefined {
  const unique = dedupe(sourceIds);
  if (!unique) return undefined;
  return [...unique].sort((a, b) => a.localeCompare(b));
}

function canonicalizeCardIds(
  cardIds: string[] | undefined,
): string[] | undefined {
  const unique = dedupe(cardIds);
  if (!unique) return undefined;
  return [...unique].sort((a, b) => a.localeCompare(b));
}

function normalizeFilters(
  filters: Record<string, DashboardFilterValue> | undefined,
): Record<string, DashboardFilterValue> | undefined {
  if (!filters) return undefined;
  const entries = Object.entries(filters)
    .filter(([, value]) => value !== undefined)
    .sort(([a], [b]) => a.localeCompare(b));

  if (!entries.length) return undefined;

  return Object.fromEntries(entries) as Record<string, DashboardFilterValue>;
}

export function normalizeDashboardDataRequest(
  request: DashboardDataRequest,
): DashboardDataRequest {
  return {
    filters: normalizeFilters(request.filters),
    time_window: request.time_window,
    time_range: request.time_range,
    timezone: request.timezone,
    source_ids: canonicalizeSourceIds(request.source_ids),
    card_ids: canonicalizeCardIds(request.card_ids),
    include_meta: request.include_meta ?? true,
    request_id: request.request_id,
  };
}

export function useDashboardSpec(dashboardRef: string) {
  return useQuery({
    queryKey: ["dashboards", dashboardRef, "spec"],
    queryFn: () => fetchDashboardSpec(dashboardRef),
    enabled: Boolean(dashboardRef),
    staleTime: 60000,
  });
}

export function useDashboardMetadata(dashboardRef: string) {
  return useQuery<DashboardMetadataResponse>({
    queryKey: ["dashboards", dashboardRef, "metadata"],
    queryFn: () => fetchDashboardMetadata(dashboardRef),
    enabled: Boolean(dashboardRef),
    staleTime: 30000,
  });
}

export function useDashboardList() {
  return useQuery<DashboardListItem[]>({
    queryKey: ["dashboards", "list"],
    queryFn: fetchDashboardList,
    staleTime: 60000,
  });
}

export function useDashboardSourceCatalog() {
  return useQuery<DashboardSourceCatalogResponse>({
    queryKey: ["dashboards", "source-catalog"],
    queryFn: fetchDashboardSourceCatalog,
    staleTime: 5 * 60 * 1000,
  });
}

interface DashboardDataQueryOptions {
  dashboardRef: string;
  request: DashboardDataRequest;
  enabled?: boolean;
  refreshSeconds?: number;
}

export function useDashboardData({
  dashboardRef,
  request,
  enabled = true,
  refreshSeconds,
}: DashboardDataQueryOptions) {
  const normalized = normalizeDashboardDataRequest(request);

  return useQuery({
    queryKey: ["dashboards", dashboardRef, "data", normalized],
    queryFn: () => fetchDashboardData(dashboardRef, normalized),
    enabled: enabled && Boolean(dashboardRef),
    staleTime: 10000,
    refetchInterval:
      refreshSeconds && refreshSeconds >= 5 ? refreshSeconds * 1000 : false,
    placeholderData: keepPreviousData,
  });
}

export function usePreviewDashboard() {
  return useMutation({
    mutationFn: async (request: DashboardPreviewRequest) => {
      return previewDashboard({
        ...request,
        request: normalizeDashboardDataRequest(request.request),
        fallback_request: request.fallback_request
          ? normalizeDashboardDataRequest(request.fallback_request)
          : undefined,
      });
    },
  });
}

export function useCreateDashboard() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (request: DashboardCreateRequest) => createDashboard(request),
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ["dashboards", "list"] });
      queryClient.setQueryData(["dashboards", data.ref, "metadata"], data);
      queryClient.invalidateQueries({ queryKey: ["dashboards", data.ref, "spec"] });
    },
  });
}

export function useUpdateDashboard() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      request,
    }: {
      ref: string;
      request: DashboardUpdateRequest;
    }) => updateDashboard(ref, request),
    onSuccess: (data, variables) => {
      queryClient.invalidateQueries({ queryKey: ["dashboards", "list"] });
      queryClient.setQueryData(["dashboards", data.ref, "metadata"], data);
      queryClient.invalidateQueries({ queryKey: ["dashboards", data.ref, "spec"] });
      queryClient.invalidateQueries({ queryKey: ["dashboards", variables.ref, "data"] });
    },
  });
}

export function useDeleteDashboard() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (ref: string) => deleteDashboard(ref),
    onSuccess: (_, ref) => {
      queryClient.invalidateQueries({ queryKey: ["dashboards", "list"] });
      queryClient.removeQueries({ queryKey: ["dashboards", ref] });
    },
  });
}

export function useCloneDashboard() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      ref,
      request,
    }: {
      ref: string;
      request: DashboardCloneRequest;
    }) => cloneDashboard(ref, request),
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ["dashboards", "list"] });
      queryClient.setQueryData(["dashboards", data.ref, "metadata"], data);
      queryClient.invalidateQueries({ queryKey: ["dashboards", data.ref, "spec"] });
    },
  });
}

export function getDashboardFilterDefaults(
  spec: DashboardSpec | undefined,
): Record<string, DashboardFilterValue> {
  if (!spec?.filters?.length) {
    return {};
  }

  return spec.filters.reduce<Record<string, DashboardFilterValue>>((acc, filter) => {
    if (filter.default !== undefined) {
      acc[filter.id] = filter.default;
    }
    return acc;
  }, {});
}

export function isDashboardDocumentDirty(
  baseline: DashboardAuthoringDocument | null,
  current: DashboardAuthoringDocument | null,
): boolean {
  if (!baseline || !current) {
    return false;
  }
  return JSON.stringify(baseline) !== JSON.stringify(current);
}
