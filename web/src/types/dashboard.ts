export type DashboardPrimitive = string | number | boolean | null;

export type DashboardFilterValue =
  | DashboardPrimitive
  | string[]
  | number[];

export interface DashboardDefaults {
  timezone?: string;
  refresh_seconds?: number;
  time_window?: string;
}

export interface DashboardBreakpoint {
  min_width: number;
  columns: number;
}

export interface DashboardLayout {
  columns: number;
  row_height: number;
  gap: number;
  breakpoints: Record<string, DashboardBreakpoint>;
}

export interface DashboardFilterSpec {
  id: string;
  type:
    | "pack_ref"
    | "action_ref"
    | "queue_ref"
    | "trigger_ref"
    | "time_window"
    | "enum"
    | "text"
    | "number"
    | "boolean"
    | string;
  label: string;
  default?: DashboardFilterValue;
  options?: DashboardFilterValue[];
}

export interface DashboardDataSource {
  type: string;
  params: Record<string, unknown>;
}

export interface DashboardGridRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type DashboardVisualizationType =
  | "stat"
  | "kpi"
  | "timeseries"
  | "stacked_timeseries"
  | "gauge"
  | "table"
  | string;

export interface DashboardGaugeBand {
  from: number;
  to: number;
  level: "good" | "warning" | "bad" | string;
  color?: string;
}

export interface DashboardCardVisualization {
  type: DashboardVisualizationType;
  value_field?: string;
  x_field?: string;
  y_field?: string;
  series_field?: string;
  format?: "integer" | "float" | "percent" | "duration_ms" | "relative_time" | string;
  legend?: boolean;
  mode?: "high_is_bad" | "low_is_bad" | "target_range" | string;
  min?: number;
  max?: number;
  bands?: DashboardGaugeBand[];
}

export interface DashboardCardSpec {
  id: string;
  title: string;
  subtitle?: string;
  source: string;
  visualization: DashboardCardVisualization;
  position: Record<string, DashboardGridRect>;
}

export interface DashboardSpec {
  version: number;
  kind: "dashboard" | string;
  ref: string;
  label: string;
  description?: string;
  tags?: string[];
  defaults?: DashboardDefaults;
  layout: DashboardLayout;
  filters?: DashboardFilterSpec[];
  data_sources: Record<string, DashboardDataSource>;
  cards: DashboardCardSpec[];
  revision?: number;
}

export interface DashboardMetadataResponse {
  id: number;
  ref: string;
  label: string;
  description?: string;
  revision: number;
  spec_version: number;
  spec: DashboardSpec;
  tags: string[];
  created: string;
  updated: string;
}

export interface DashboardTimeRange {
  start: string;
  end: string;
}

export interface DashboardDataRequest {
  filters?: Record<string, DashboardFilterValue>;
  time_window?: string;
  time_range?: DashboardTimeRange;
  timezone?: string;
  source_ids?: string[];
  card_ids?: string[];
  include_meta?: boolean;
  request_id?: string;
}

export type DashboardSourceStatus =
  | "ok"
  | "empty"
  | "partial"
  | "stale"
  | "forbidden"
  | "invalid"
  | "error";

export interface DashboardSourceError {
  code: string;
  message: string;
  retryable: boolean;
  details: Record<string, unknown> | null;
}

export interface DashboardSourceMeta {
  authorization_mode: "operator_global" | "identity_filtered";
  freshness_mode:
    | "raw_only"
    | "aggregate_only"
    | "aggregate_plus_tail"
    | "raw_only_fallback";
  aggregate_watermark: string | null;
  cache_hit: boolean;
  bucket_size: string | null;
  truncated: boolean;
  unit_hints: Record<string, string>;
  ordering: string[];
  authorized_refs: Record<string, unknown> | null;
}

export interface DashboardSourceResult {
  source_id: string;
  source_type: string;
  status: DashboardSourceStatus;
  data: Record<string, unknown> | Array<Record<string, unknown>> | null;
  meta: DashboardSourceMeta;
  error: DashboardSourceError | null;
}

export interface DashboardDataResponse {
  contract_version: number;
  dashboard_ref: string;
  dashboard_revision: number;
  spec_version: number;
  resolved_at: string;
  request_id: string | null;
  effective_time_range: {
    start: string;
    end: string;
    timezone: string;
  };
  partial: boolean;
  sources: DashboardSourceResult[];
}
