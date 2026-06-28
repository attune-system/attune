export type DashboardPrimitive = string | number | boolean | null;

export type DashboardScopeType =
  | "global"
  | "pack"
  | "identity"
  | "tenant"
  | string;

export type DashboardVisibility = "public" | "pack" | "private" | string;

export type DashboardFilterValue = DashboardPrimitive | string[] | number[];

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
  format?:
    | "integer"
    | "float"
    | "percent"
    | "duration_ms"
    | "relative_time"
    | string;
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

export type DashboardSpecRecord = DashboardSpec & Record<string, unknown>;

export interface DashboardMetadataResponse {
  id: number;
  ref: string;
  scope_type: DashboardScopeType;
  scope_ref: string;
  pack?: number | null;
  owner_identity?: number | null;
  visibility: DashboardVisibility;
  is_adhoc?: boolean;
  label: string;
  description?: string;
  enabled?: boolean;
  is_default_home?: boolean;
  revision: number;
  spec_version: number;
  spec: DashboardSpecRecord;
  tags: string[];
  created: string;
  updated: string;
}

export interface DashboardListItem {
  id: number;
  ref: string;
  label: string;
  description?: string;
  scope_type: DashboardScopeType;
  scope_ref: string;
  visibility: DashboardVisibility;
  is_default_home: boolean;
  revision: number;
  tags: string[];
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

export interface DashboardAuthoringDocument {
  id?: number;
  revision?: number;
  ref: string;
  label: string;
  description?: string;
  scope_type: DashboardScopeType;
  scope_ref: string;
  visibility: DashboardVisibility;
  enabled: boolean;
  is_default_home: boolean;
  spec_version: number;
  tags: string[];
  spec: DashboardSpecRecord;
  extra_spec_fields?: Record<string, unknown>;
}

export interface DashboardCreateRequest {
  ref: string;
  label: string;
  description?: string;
  scope_type: DashboardScopeType;
  scope_ref: string;
  visibility: DashboardVisibility;
  enabled: boolean;
  is_default_home: boolean;
  spec_version: number;
  spec: DashboardSpecRecord;
  tags: string[];
}

export interface DashboardUpdateRequest {
  label?: string;
  description?: { op: "set"; value: string } | { op: "clear" };
  scope_type?: DashboardScopeType;
  scope_ref?: string;
  visibility?: DashboardVisibility;
  enabled?: boolean;
  is_default_home?: boolean;
  spec_version?: number;
  spec?: DashboardSpecRecord;
  tags?: string[];
  expected_revision?: number;
}

export interface DashboardCloneRequest {
  ref: string;
  label?: string;
  description?: string;
  scope_type?: DashboardScopeType;
  scope_ref?: string;
  visibility?: DashboardVisibility;
  enabled?: boolean;
  is_default_home?: boolean;
  spec_version?: number;
  tags?: string[];
}

export interface DashboardSourceParamDefinition {
  name: string;
  required: boolean;
  input: "text" | "number" | "boolean";
}

export interface DashboardSourceContract {
  source_type: string;
  availability: "available_now" | "partial" | "planned" | string;
  authorization_basis: string;
  default_freshness_mode: string;
  params: DashboardSourceParamDefinition[];
  ordering: string[];
  response_shape: string;
  notes?: string;
}

export interface DashboardSourceCatalogResponse {
  source: "api" | "fallback";
  contracts: DashboardSourceContract[];
}

export interface DashboardPreviewRequest {
  dashboard: DashboardCreateRequest;
  request: DashboardDataRequest;
  fallback_dashboard_ref?: string;
  fallback_request?: DashboardDataRequest;
}

export interface DashboardPreviewResponse {
  mode: "draft" | "published_fallback";
  endpoint_available: boolean;
  warning?: string;
  data: DashboardDataResponse;
}

const KNOWN_SPEC_KEYS = new Set<string>([
  "version",
  "kind",
  "ref",
  "label",
  "description",
  "tags",
  "defaults",
  "layout",
  "filters",
  "data_sources",
  "cards",
  "revision",
]);

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function sortStrings(values: string[] | undefined): string[] {
  return [...(values ?? [])].sort((a, b) => a.localeCompare(b));
}

function sortObjectKeys<T>(value: Record<string, T>): Record<string, T> {
  return Object.fromEntries(
    Object.entries(value).sort(([left], [right]) => left.localeCompare(right)),
  ) as Record<string, T>;
}

function normalizeScopeRefForRequest(
  scopeType: DashboardScopeType,
  scopeRef: string,
): string {
  const trimmed = scopeRef.trim();
  if (scopeType !== "pack") {
    return trimmed;
  }
  return trimmed
    .toLowerCase()
    .replace(/[^a-z0-9_.-]+/g, "_")
    .replace(/^_+|_+$/g, "");
}

export function extractDashboardExtraSpecFields(
  spec: DashboardSpecRecord,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(spec).filter(([key]) => !KNOWN_SPEC_KEYS.has(key)),
  );
}

export function dashboardMetadataToDocument(
  metadata: DashboardMetadataResponse,
  options?: { cloneFromExisting?: boolean },
): DashboardAuthoringDocument {
  const cloneFromExisting = options?.cloneFromExisting === true;
  const rawSpec = cloneJson(metadata.spec ?? ({} as DashboardSpecRecord));
  const extra_spec_fields = extractDashboardExtraSpecFields(rawSpec);

  const spec: DashboardSpecRecord = {
    ...cloneJson(rawSpec),
    ref: cloneFromExisting ? "" : metadata.ref,
    label: cloneFromExisting ? `${metadata.label} Copy` : metadata.label,
    description: metadata.description ?? rawSpec.description,
    tags: sortStrings(metadata.tags ?? rawSpec.tags ?? []),
    defaults: cloneJson(rawSpec.defaults ?? {}),
    layout: cloneJson(rawSpec.layout),
    filters: cloneJson(rawSpec.filters ?? []),
    data_sources: cloneJson(rawSpec.data_sources ?? {}),
    cards: cloneJson(rawSpec.cards ?? []),
    revision: cloneFromExisting ? undefined : metadata.revision,
  };

  return {
    id: cloneFromExisting ? undefined : metadata.id,
    revision: cloneFromExisting ? undefined : metadata.revision,
    ref: cloneFromExisting ? "" : metadata.ref,
    label: cloneFromExisting ? `${metadata.label} Copy` : metadata.label,
    description: metadata.description,
    scope_type: metadata.scope_type,
    scope_ref: metadata.scope_ref,
    visibility: metadata.visibility,
    enabled: metadata.enabled ?? true,
    is_default_home: cloneFromExisting ? false : (metadata.is_default_home ?? false),
    spec_version: metadata.spec_version,
    tags: sortStrings(metadata.tags),
    spec,
    extra_spec_fields,
  };
}

export function createEmptyDashboardDocument(): DashboardAuthoringDocument {
  const spec: DashboardSpecRecord = {
    version: 1,
    kind: "dashboard",
    ref: "",
    label: "",
    description: "",
    tags: [],
    defaults: {
      timezone: "UTC",
      refresh_seconds: 15,
      time_window: "24h",
    },
    layout: {
      columns: 12,
      row_height: 44,
      gap: 12,
      breakpoints: {
        lg: { min_width: 1280, columns: 12 },
        sm: { min_width: 0, columns: 4 },
      },
    },
    filters: [],
    data_sources: {},
    cards: [],
  };

  return {
    ref: "",
    label: "",
    description: "",
    scope_type: "global",
    scope_ref: "global",
    visibility: "public",
    enabled: true,
    is_default_home: false,
    spec_version: 1,
    tags: [],
    spec,
    extra_spec_fields: {},
  };
}

export function dashboardDocumentToSpec(
  document: DashboardAuthoringDocument,
): DashboardSpecRecord {
  const base = cloneJson(document.spec);
  const extra = cloneJson(document.extra_spec_fields ?? {});

  const normalized: DashboardSpecRecord = {
    ...extra,
    version: Number(base.version ?? 1),
    kind: typeof base.kind === "string" ? base.kind : "dashboard",
    ref: document.ref,
    label: document.label,
    description: document.description || undefined,
    tags: sortStrings(document.tags),
    defaults: cloneJson(base.defaults ?? {}),
    layout: {
      columns: Number(base.layout?.columns ?? 12),
      row_height: Number(base.layout?.row_height ?? 44),
      gap: Number(base.layout?.gap ?? 12),
      breakpoints: sortObjectKeys(cloneJson(base.layout?.breakpoints ?? {})),
    },
    filters: cloneJson(base.filters ?? []),
    data_sources: sortObjectKeys(cloneJson(base.data_sources ?? {})),
    cards: cloneJson(base.cards ?? []),
  };

  if (document.revision !== undefined) {
    normalized.revision = document.revision;
  }

  return normalized;
}

export function dashboardDocumentToCreateRequest(
  document: DashboardAuthoringDocument,
): DashboardCreateRequest {
  const normalizedScopeRef = normalizeScopeRefForRequest(
    document.scope_type,
    document.scope_ref,
  );
  return {
    ref: document.ref,
    label: document.label,
    description: document.description || undefined,
    scope_type: document.scope_type,
    scope_ref: normalizedScopeRef,
    visibility: document.visibility,
    enabled: document.enabled,
    is_default_home: document.is_default_home,
    spec_version: document.spec_version,
    spec: dashboardDocumentToSpec(document),
    tags: sortStrings(document.tags),
  };
}

export function dashboardDocumentToUpdateRequest(
  document: DashboardAuthoringDocument,
): DashboardUpdateRequest {
  const normalizedScopeRef = normalizeScopeRefForRequest(
    document.scope_type,
    document.scope_ref,
  );
  const trimmedDescription = (document.description ?? "").trim();
  return {
    label: document.label,
    description: trimmedDescription
      ? { op: "set", value: trimmedDescription }
      : { op: "clear" },
    scope_type: document.scope_type,
    scope_ref: normalizedScopeRef,
    visibility: document.visibility,
    enabled: document.enabled,
    is_default_home: document.is_default_home,
    spec_version: document.spec_version,
    spec: dashboardDocumentToSpec(document),
    tags: sortStrings(document.tags),
    expected_revision: document.revision,
  };
}

export function dashboardDocumentToCloneRequest(
  document: DashboardAuthoringDocument,
): DashboardCloneRequest {
  const normalizedScopeRef = normalizeScopeRefForRequest(
    document.scope_type,
    document.scope_ref,
  );
  return {
    ref: document.ref,
    label: document.label,
    description: document.description || undefined,
    scope_type: document.scope_type,
    scope_ref: normalizedScopeRef,
    visibility: document.visibility,
    enabled: document.enabled,
    is_default_home: false,
    spec_version: document.spec_version,
    tags: sortStrings(document.tags),
  };
}

export function dashboardDocumentToYamlObject(
  document: DashboardAuthoringDocument,
): Record<string, unknown> {
  const spec = dashboardDocumentToSpec(document);
  const yamlObject: Record<string, unknown> = {
    version: spec.version,
    kind: spec.kind,
    ref: document.ref,
    label: document.label,
    description: document.description || undefined,
    scope_type: document.scope_type,
    scope_ref: document.scope_ref,
    visibility: document.visibility,
    is_default_home: document.is_default_home,
    enabled: document.enabled,
    spec_version: document.spec_version,
    defaults: spec.defaults,
    layout: spec.layout,
    filters: spec.filters,
    data_sources: spec.data_sources,
    cards: spec.cards,
    tags: sortStrings(document.tags),
  };

  for (const [key, value] of Object.entries(document.extra_spec_fields ?? {})) {
    if (!(key in yamlObject)) {
      yamlObject[key] = value;
    }
  }

  return yamlObject;
}

export function cloneDashboardDocument(
  document: DashboardAuthoringDocument,
): DashboardAuthoringDocument {
  return cloneJson(document);
}
