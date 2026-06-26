import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { fetchDashboardData, fetchDashboardSpec } from "@/lib/dashboard-client";
import type {
  DashboardDataRequest,
  DashboardFilterValue,
  DashboardSpec,
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
    // Backend contract: source_ids are treated as an unordered selector and
    // responses are emitted in canonical source_id order.
    source_ids: canonicalizeSourceIds(request.source_ids),
    // card_ids are also membership selectors; request order does not affect
    // server-side source resolution.
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
