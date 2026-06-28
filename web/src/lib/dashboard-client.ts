import { apiClient } from "@/lib/api-client";
import { DASHBOARD_SOURCE_CATALOG_FALLBACK } from "@/lib/dashboard-source-catalog";
import type {
  DashboardCloneRequest,
  DashboardCreateRequest,
  DashboardDataRequest,
  DashboardDataResponse,
  DashboardListItem,
  DashboardMetadataResponse,
  DashboardPreviewRequest,
  DashboardPreviewResponse,
  DashboardSourceCatalogResponse,
  DashboardSourceContract,
  DashboardSourceParamDefinition,
  DashboardSpec,
  DashboardSpecRecord,
  DashboardUpdateRequest,
} from "@/types/dashboard";

interface ApiEnvelope<T> {
  data: T;
}

export interface DashboardClientErrorInfo {
  status?: number;
  message: string;
  conflict: boolean;
  unsupported: boolean;
}

function isApiEnvelope<T>(payload: unknown): payload is ApiEnvelope<T> {
  return typeof payload === "object" && payload !== null && "data" in payload;
}

function unwrapApi<T>(payload: T | ApiEnvelope<T>): T {
  if (isApiEnvelope<T>(payload)) {
    return (payload as ApiEnvelope<T>).data;
  }
  return payload as T;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function toNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function inferSourceParamInput(name: string): "text" | "number" | "boolean" {
  if (["decrypt", "include_in_flight", "include_cancelled", "history"].includes(name)) {
    return "boolean";
  }
  if (["assigned_to", "sla_target_seconds", "worker_id"].includes(name)) {
    return "number";
  }
  return "text";
}

function parseDashboardSpec(payload: unknown): DashboardSpecRecord {
  const assertDashboardCoreShape = (candidate: Record<string, unknown>) => {
    if (!isObject(candidate.layout)) {
      throw new Error("Dashboard spec is missing layout");
    }
    if (!Array.isArray(candidate.cards)) {
      throw new Error("Dashboard spec is missing cards");
    }
    if (!isObject(candidate.data_sources)) {
      throw new Error("Dashboard spec is missing data_sources");
    }
    if (typeof candidate.version !== "number") {
      throw new Error("Dashboard spec is missing version");
    }
    if (typeof candidate.kind !== "string") {
      throw new Error("Dashboard spec is missing kind");
    }
  };

  if (!isObject(payload)) {
    throw new Error("Dashboard spec response is not an object");
  }

  assertDashboardCoreShape(payload);
  return payload as DashboardSpecRecord;
}

function parseDashboardMetadata(payload: unknown): DashboardMetadataResponse {
  if (!isObject(payload)) {
    throw new Error("Dashboard metadata response is not an object");
  }
  if (typeof payload.ref !== "string") {
    throw new Error("Dashboard metadata response is missing ref");
  }
  if (typeof payload.label !== "string") {
    throw new Error("Dashboard metadata response is missing label");
  }
  if (typeof payload.revision !== "number") {
    throw new Error("Dashboard metadata response is missing revision");
  }
  if (!isObject(payload.spec)) {
    throw new Error("Dashboard metadata response is missing spec");
  }

  return {
    ...((payload as unknown) as DashboardMetadataResponse),
    spec: parseDashboardSpec({
      ...(payload.spec as DashboardSpecRecord),
      ref: payload.ref,
      label: payload.label,
      description:
        typeof payload.description === "string"
          ? payload.description
          : (payload.spec as DashboardSpecRecord).description,
      tags: Array.isArray(payload.tags)
        ? payload.tags
        : ((payload.spec as DashboardSpecRecord).tags ?? []),
      revision: payload.revision,
    }),
  };
}

function parseDashboardDataResponse(payload: unknown): DashboardDataResponse {
  if (!isObject(payload) || !Array.isArray(payload.sources)) {
    throw new Error("Dashboard data response is missing sources");
  }
  return (payload as unknown) as DashboardDataResponse;
}

function parseSourceParamDefinition(
  name: string,
  required: boolean,
  candidate?: Record<string, unknown>,
): DashboardSourceParamDefinition {
  const rawInput = candidate?.input;
  const input =
    rawInput === "number" || rawInput === "boolean" || rawInput === "text"
      ? rawInput
      : inferSourceParamInput(name);
  return { name, required, input };
}

function normalizeSourceContract(payload: unknown): DashboardSourceContract | null {
  if (!isObject(payload) || typeof payload.source_type !== "string") {
    return null;
  }

  if (Array.isArray(payload.params)) {
    return {
      source_type: payload.source_type,
      availability:
        typeof payload.availability === "string"
          ? payload.availability
          : "available_now",
      authorization_basis:
        typeof payload.authorization_basis === "string"
          ? payload.authorization_basis
          : "dashboards",
      default_freshness_mode:
        typeof payload.default_freshness_mode === "string"
          ? payload.default_freshness_mode
          : "raw_only",
      params: payload.params
        .filter(isObject)
        .map((param) =>
          parseSourceParamDefinition(
            typeof param.name === "string" ? param.name : "param",
            Boolean(param.required),
            param,
          ),
        ),
      ordering: Array.isArray(payload.ordering)
        ? payload.ordering.filter((item): item is string => typeof item === "string")
        : [],
      response_shape:
        typeof payload.response_shape === "string" ? payload.response_shape : "array",
      notes: typeof payload.notes === "string" ? payload.notes : undefined,
    };
  }

  const paramSchema = isObject(payload.param_schema) ? payload.param_schema : {};
  const required = Array.isArray(paramSchema.required)
    ? paramSchema.required.filter((item): item is string => typeof item === "string")
    : [];
  const optional = Array.isArray(paramSchema.optional)
    ? paramSchema.optional.filter((item): item is string => typeof item === "string")
    : [];

  return {
    source_type: payload.source_type,
    availability:
      typeof payload.availability === "string"
        ? payload.availability
        : "available_now",
    authorization_basis:
      typeof payload.authorization_basis === "string"
        ? payload.authorization_basis
        : "dashboards",
    default_freshness_mode:
      typeof payload.default_freshness_mode === "string"
        ? payload.default_freshness_mode
        : "raw_only",
    params: [
      ...required.map((name) => parseSourceParamDefinition(name, true)),
      ...optional.map((name) => parseSourceParamDefinition(name, false)),
    ],
    ordering: Array.isArray(payload.ordering)
      ? payload.ordering.filter((item): item is string => typeof item === "string")
      : [],
    response_shape:
      typeof payload.response_shape === "string" ? payload.response_shape : "array",
    notes: typeof payload.notes === "string" ? payload.notes : undefined,
  };
}

function parseSourceCatalogResponse(payload: unknown): DashboardSourceCatalogResponse {
  const data = unwrapApi(payload as DashboardSourceCatalogResponse | ApiEnvelope<DashboardSourceCatalogResponse>);
  if (Array.isArray(data)) {
    return {
      source: "api",
      contracts: data
        .map(normalizeSourceContract)
        .filter((item): item is DashboardSourceContract => item !== null),
    };
  }

  if (isObject(data) && Array.isArray(data.contracts)) {
    return {
      source: data.source === "fallback" ? "fallback" : "api",
      contracts: data.contracts
        .map(normalizeSourceContract)
        .filter((item): item is DashboardSourceContract => item !== null),
    };
  }

  if (isObject(data)) {
    const contracts = Object.values(data)
      .map(normalizeSourceContract)
      .filter((item): item is DashboardSourceContract => item !== null);
    if (contracts.length > 0) {
      return { source: "api", contracts };
    }
  }

  throw new Error("Dashboard source catalog response could not be parsed");
}

function extractErrorMessage(error: unknown, fallback: string): string {
  if (isObject(error)) {
    const response = isObject(error.response) ? error.response : undefined;
    const data = isObject(response?.data) ? response.data : undefined;
    if (typeof data?.message === "string") {
      return data.message;
    }
    if (typeof error.message === "string") {
      return error.message;
    }
  }
  if (error instanceof Error) {
    return error.message;
  }
  return fallback;
}

function extractStatus(error: unknown): number | undefined {
  if (isObject(error)) {
    const response = isObject(error.response) ? error.response : undefined;
    return typeof response?.status === "number" ? response.status : undefined;
  }
  return undefined;
}

export function getDashboardClientErrorInfo(
  error: unknown,
  fallback = "Dashboard request failed",
): DashboardClientErrorInfo {
  const status = extractStatus(error);
  return {
    status,
    message: extractErrorMessage(error, fallback),
    conflict: status === 409,
    unsupported: status === 404 || status === 405 || status === 501,
  };
}

export async function fetchDashboardSpec(ref: string): Promise<DashboardSpec> {
  const metadata = await fetchDashboardMetadata(ref);
  return metadata.spec;
}

export async function fetchDashboardMetadata(
  ref: string,
): Promise<DashboardMetadataResponse> {
  const response = await apiClient.get<
    DashboardMetadataResponse | ApiEnvelope<DashboardMetadataResponse>
  >(`/api/v1/dashboards/${encodeURIComponent(ref)}`);
  return parseDashboardMetadata(unwrapApi(response.data));
}

export async function fetchDashboardList(): Promise<DashboardListItem[]> {
  const response = await apiClient.get<
    DashboardListItem[] | ApiEnvelope<DashboardListItem[]>
  >("/api/v1/dashboards");
  return unwrapApi(response.data);
}

export async function fetchDashboardData(
  ref: string,
  request: DashboardDataRequest,
): Promise<DashboardDataResponse> {
  const response = await apiClient.post<
    DashboardDataResponse | ApiEnvelope<DashboardDataResponse>
  >(`/api/v1/dashboards/${encodeURIComponent(ref)}/data`, request);
  return parseDashboardDataResponse(unwrapApi(response.data));
}

export async function fetchDashboardSourceCatalog(): Promise<DashboardSourceCatalogResponse> {
  for (const path of [
    "/api/v1/dashboards/source-catalog",
    "/api/v1/dashboard-sources",
  ]) {
    try {
      const response = await apiClient.get(path);
      const parsed = parseSourceCatalogResponse(response.data);
      if (parsed.contracts.length > 0) {
        return parsed;
      }
      // Treat empty catalogs as unsupported for authoring; keep UI usable via fallback.
      return DASHBOARD_SOURCE_CATALOG_FALLBACK;
    } catch (error) {
      const info = getDashboardClientErrorInfo(error);
      if (info.unsupported) {
        continue;
      }
      // Any parse/shape/network issue should degrade to fallback so source_type select is never empty.
      return DASHBOARD_SOURCE_CATALOG_FALLBACK;
    }
  }

  return DASHBOARD_SOURCE_CATALOG_FALLBACK;
}

export async function createDashboard(
  request: DashboardCreateRequest,
): Promise<DashboardMetadataResponse> {
  const response = await apiClient.post<
    DashboardMetadataResponse | ApiEnvelope<DashboardMetadataResponse>
  >("/api/v1/dashboards", request);
  return parseDashboardMetadata(unwrapApi(response.data));
}

export async function updateDashboard(
  ref: string,
  request: DashboardUpdateRequest,
): Promise<DashboardMetadataResponse> {
  const response = await apiClient.put<
    DashboardMetadataResponse | ApiEnvelope<DashboardMetadataResponse>
  >(`/api/v1/dashboards/${encodeURIComponent(ref)}`, request);
  return parseDashboardMetadata(unwrapApi(response.data));
}

export async function deleteDashboard(ref: string): Promise<void> {
  await apiClient.delete(`/api/v1/dashboards/${encodeURIComponent(ref)}`);
}

export async function cloneDashboard(
  ref: string,
  request: DashboardCloneRequest,
): Promise<DashboardMetadataResponse> {
  const response = await apiClient.post<
    DashboardMetadataResponse | ApiEnvelope<DashboardMetadataResponse>
  >(`/api/v1/dashboards/${encodeURIComponent(ref)}/clone`, request);
  return parseDashboardMetadata(unwrapApi(response.data));
}

export async function previewDashboard(
  request: DashboardPreviewRequest,
): Promise<DashboardPreviewResponse> {
  try {
    const response = await apiClient.post<
      DashboardDataResponse | ApiEnvelope<DashboardDataResponse>
    >("/api/v1/dashboards/preview", {
      dashboard: request.dashboard,
      data_request: request.request,
    });

    return {
      mode: "draft",
      endpoint_available: true,
      data: parseDashboardDataResponse(unwrapApi(response.data)),
    };
  } catch (error) {
    const info = getDashboardClientErrorInfo(error, "Failed to preview dashboard");
    if (
      info.unsupported &&
      request.fallback_dashboard_ref &&
      request.fallback_request
    ) {
      const data = await fetchDashboardData(
        request.fallback_dashboard_ref,
        request.fallback_request,
      );
      return {
        mode: "published_fallback",
        endpoint_available: false,
        warning:
          "Draft preview endpoint is unavailable. Showing the published dashboard instead.",
        data,
      };
    }
    throw error;
  }
}

export function pickSourceContract(
  catalog: DashboardSourceCatalogResponse | undefined,
  sourceType: string,
): DashboardSourceContract | undefined {
  return catalog?.contracts.find((contract) => contract.source_type === sourceType);
}

export function previewGridColumns(spec: DashboardSpecRecord, breakpoint: string): number {
  return toNumber(spec.layout.breakpoints?.[breakpoint]?.columns, spec.layout.columns);
}
