import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { apiClient } from "@/lib/api-client";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/**
 * A single data point in an hourly time series.
 */
export interface TimeSeriesPoint {
  /** Start of the 1-hour bucket (ISO 8601) */
  bucket: string;
  /** Series label (e.g., status name, action ref). Null for aggregate totals. */
  label: string | null;
  /** The count value for this bucket */
  value: number;
}

/**
 * Failure rate summary over a time range.
 */
export interface FailureRateSummary {
  since: string;
  until: string;
  total_terminal: number;
  failed_count: number;
  timeout_count: number;
  completed_count: number;
  failure_rate_pct: number;
}

/**
 * Combined dashboard analytics payload returned by GET /api/v1/analytics/dashboard.
 */
export interface DashboardAnalytics {
  since: string;
  until: string;
  execution_throughput: TimeSeriesPoint[];
  execution_status: TimeSeriesPoint[];
  event_volume: TimeSeriesPoint[];
  enforcement_volume: TimeSeriesPoint[];
  worker_status: TimeSeriesPoint[];
  failure_rate: FailureRateSummary;
}

/**
 * A generic time-series response (used by the individual endpoints).
 */
export interface TimeSeriesResponse {
  since: string;
  until: string;
  data: TimeSeriesPoint[];
}

/**
 * Query parameters for analytics requests.
 */
export interface AnalyticsQueryParams {
  /** Start of time range (ISO 8601). Defaults to 24 hours ago on the server. */
  since?: string;
  /** End of time range (ISO 8601). Defaults to now on the server. */
  until?: string;
  /** Number of hours to look back from now (alternative to since/until). */
  hours?: number;
}

// ---------------------------------------------------------------------------
// Fetch helpers
// ---------------------------------------------------------------------------

async function fetchDashboardAnalytics(
  params: AnalyticsQueryParams,
): Promise<DashboardAnalytics> {
  const queryParams: Record<string, string | number> = {};
  if (params.since) queryParams.since = params.since;
  if (params.until) queryParams.until = params.until;
  if (params.hours) queryParams.hours = params.hours;

  const response = await apiClient.get<{ data: DashboardAnalytics }>(
    "/api/v1/analytics/dashboard",
    { params: queryParams },
  );

  return response.data.data;
}

async function fetchTimeSeries(
  path: string,
  params: AnalyticsQueryParams,
): Promise<TimeSeriesResponse> {
  const queryParams: Record<string, string | number> = {};
  if (params.since) queryParams.since = params.since;
  if (params.until) queryParams.until = params.until;
  if (params.hours) queryParams.hours = params.hours;

  const response = await apiClient.get<{ data: TimeSeriesResponse }>(
    `/api/v1/analytics/${path}`,
    { params: queryParams },
  );

  return response.data.data;
}

async function fetchFailureRate(
  params: AnalyticsQueryParams,
): Promise<FailureRateSummary> {
  const queryParams: Record<string, string | number> = {};
  if (params.since) queryParams.since = params.since;
  if (params.until) queryParams.until = params.until;
  if (params.hours) queryParams.hours = params.hours;

  const response = await apiClient.get<{ data: FailureRateSummary }>(
    "/api/v1/analytics/executions/failure-rate",
    { params: queryParams },
  );

  return response.data.data;
}

// ---------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------

/**
 * Fetch the combined dashboard analytics payload.
 *
 * This is the recommended hook for the dashboard page — it returns all
 * key metrics in a single API call to avoid multiple round-trips.
 */
export function useDashboardAnalytics(params: AnalyticsQueryParams = {}) {
  return useQuery({
    queryKey: ["analytics", "dashboard", params],
    queryFn: () => fetchDashboardAnalytics(params),
    staleTime: 60000, // 1 minute — aggregates don't change frequently
    refetchInterval: 120000, // auto-refresh every 2 minutes
    placeholderData: keepPreviousData,
  });
}

/**
 * Fetch execution status transitions over time.
 */
export function useExecutionStatusAnalytics(params: AnalyticsQueryParams = {}) {
  return useQuery({
    queryKey: ["analytics", "executions", "status", params],
    queryFn: () => fetchTimeSeries("executions/status", params),
    staleTime: 60000,
    placeholderData: keepPreviousData,
  });
}

/**
 * Fetch execution throughput over time.
 */
export function useExecutionThroughputAnalytics(
  params: AnalyticsQueryParams = {},
) {
  return useQuery({
    queryKey: ["analytics", "executions", "throughput", params],
    queryFn: () => fetchTimeSeries("executions/throughput", params),
    staleTime: 60000,
    placeholderData: keepPreviousData,
  });
}

/**
 * Fetch execution failure rate summary.
 */
export function useFailureRateAnalytics(params: AnalyticsQueryParams = {}) {
  return useQuery({
    queryKey: ["analytics", "executions", "failure-rate", params],
    queryFn: () => fetchFailureRate(params),
    staleTime: 60000,
    placeholderData: keepPreviousData,
  });
}

/**
 * Fetch event volume over time.
 */
export function useEventVolumeAnalytics(params: AnalyticsQueryParams = {}) {
  return useQuery({
    queryKey: ["analytics", "events", "volume", params],
    queryFn: () => fetchTimeSeries("events/volume", params),
    staleTime: 60000,
    placeholderData: keepPreviousData,
  });
}

/**
 * Fetch worker status transitions over time.
 */
export function useWorkerStatusAnalytics(params: AnalyticsQueryParams = {}) {
  return useQuery({
    queryKey: ["analytics", "workers", "status", params],
    queryFn: () => fetchTimeSeries("workers/status", params),
    staleTime: 60000,
    placeholderData: keepPreviousData,
  });
}

/**
 * Fetch enforcement volume over time.
 */
export function useEnforcementVolumeAnalytics(
  params: AnalyticsQueryParams = {},
) {
  return useQuery({
    queryKey: ["analytics", "enforcements", "volume", params],
    queryFn: () => fetchTimeSeries("enforcements/volume", params),
    staleTime: 60000,
    placeholderData: keepPreviousData,
  });
}
