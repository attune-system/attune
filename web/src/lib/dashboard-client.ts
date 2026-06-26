import { apiClient } from "@/lib/api-client";
import type {
  DashboardDataRequest,
  DashboardDataResponse,
  DashboardMetadataResponse,
  DashboardSpec,
} from "@/types/dashboard";

interface ApiEnvelope<T> {
  data: T;
}

function isApiEnvelope<T>(payload: unknown): payload is ApiEnvelope<T> {
  return (
    typeof payload === "object" &&
    payload !== null &&
    "data" in payload
  );
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

function isDashboardMetadataResponse(
  payload: unknown,
): payload is DashboardMetadataResponse {
  return (
    isObject(payload) &&
    typeof payload.ref === "string" &&
    typeof payload.label === "string" &&
    typeof payload.revision === "number" &&
    "spec" in payload
  );
}

function parseDashboardSpec(payload: unknown): DashboardSpec {
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

  const assertDashboardIdentity = (candidate: Record<string, unknown>) => {
    if (typeof candidate.ref !== "string") {
      throw new Error("Dashboard spec is missing ref");
    }
    if (typeof candidate.label !== "string") {
      throw new Error("Dashboard spec is missing label");
    }
  };

  if (isDashboardMetadataResponse(payload)) {
    const rawSpec = payload.spec;
    if (!isObject(rawSpec)) {
      throw new Error("Dashboard metadata response is missing a valid spec object");
    }
    assertDashboardCoreShape(rawSpec);

    return {
      ...(rawSpec as DashboardSpec),
      ref: payload.ref,
      label: payload.label,
      description:
        payload.description ?? (rawSpec.description as string | undefined),
      tags: payload.tags ?? (rawSpec.tags as string[] | undefined),
      revision: payload.revision,
    };
  }

  if (!isObject(payload)) {
    throw new Error("Dashboard spec response is not an object");
  }
  assertDashboardCoreShape(payload);
  assertDashboardIdentity(payload);

  return payload as unknown as DashboardSpec;
}

export async function fetchDashboardSpec(ref: string): Promise<DashboardSpec> {
  const response = await apiClient.get<
    DashboardSpec | DashboardMetadataResponse | ApiEnvelope<DashboardSpec | DashboardMetadataResponse>
  >(
    `/api/v1/dashboards/${encodeURIComponent(ref)}`,
  );
  return parseDashboardSpec(unwrapApi(response.data));
}

export async function fetchDashboardData(
  ref: string,
  request: DashboardDataRequest,
): Promise<DashboardDataResponse> {
  const response = await apiClient.post<
    DashboardDataResponse | ApiEnvelope<DashboardDataResponse>
  >(`/api/v1/dashboards/${encodeURIComponent(ref)}/data`, request);
  return unwrapApi(response.data);
}
