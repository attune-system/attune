import { useQuery } from "@tanstack/react-query";
import { OpenAPI } from "@/api/core/OpenAPI";
import { request as __request } from "@/api/core/request";
import { artifactRequest } from "@/lib/artifact-request";
import { useArtifactReconciliation } from "./useArtifactReconciliation";
import type { ArtifactsService } from "@/api/services/ArtifactsService";
import type { ArtifactType as ApiArtifactType } from "@/api/models/ArtifactType";
import type { ArtifactVisibility as ApiArtifactVisibility } from "@/api/models/ArtifactVisibility";
import type { OwnerType as ApiOwnerType } from "@/api/models/OwnerType";
import type { RetentionPolicyType as ApiRetentionPolicyType } from "@/api/models/RetentionPolicyType";

export type ArtifactType = `${ApiArtifactType}`;
export type ArtifactVisibility = `${ApiArtifactVisibility}`;
export type OwnerType = `${ApiOwnerType}`;
export type RetentionPolicyType = `${ApiRetentionPolicyType}`;
export type { ArtifactSummary } from "@/api/models/ArtifactSummary";
export type { ArtifactVersionSummary } from "@/api/models/ArtifactVersionSummary";
export type ArtifactResponse = Awaited<
  ReturnType<typeof ArtifactsService.getArtifact>
>["data"];

// ============================================================================
// Search / List params
// ============================================================================

export interface ArtifactsListParams {
  cursor?: string;
  perPage?: number;
  scope?: OwnerType;
  owner?: string;
  type?: ArtifactType;
  visibility?: ArtifactVisibility;
  execution?: number;
  name?: string;
}

// ============================================================================
// Paginated list response shape
// ============================================================================

export type PaginatedArtifacts = Awaited<
  ReturnType<typeof ArtifactsService.listArtifacts>
>;

export interface ArtifactPageParams {
  cursor?: string;
  perPage?: number;
}

export type PaginatedArtifactVersions = Awaited<
  ReturnType<typeof ArtifactsService.listVersions>
>;

// ============================================================================
// Hooks
// ============================================================================

/**
 * Fetch a paginated, filterable list of all artifacts.
 *
 * Uses GET /api/v1/artifacts with query params for server-side filtering.
 */
export function useArtifactsList(
  params: ArtifactsListParams = {},
  paused = false,
) {
  useArtifactReconciliation({ scope: "list", paused });
  return useQuery({
    queryKey: ["artifacts", "list", params],
    queryFn: async ({ signal }) => {
      const query: Record<string, string> = {};
      if (params.cursor) query.cursor = params.cursor;
      if (params.perPage) query.per_page = String(params.perPage);
      if (params.scope) query.scope = params.scope;
      if (params.owner) query.owner = params.owner;
      if (params.type) query.type = params.type;
      if (params.visibility) query.visibility = params.visibility;
      if (params.execution) query.execution = String(params.execution);
      if (params.name) query.name = params.name;

      const response = await artifactRequest(
        __request<PaginatedArtifacts>(OpenAPI, {
          method: "GET",
          url: "/api/v1/artifacts",
          query,
        }),
        signal,
      );
      return response;
    },
    staleTime: 10000,
    // A paused browse still loads a newly selected page, but does not let a
    // different active subscriber's prefix invalidation refresh loaded pages.
    enabled: (query) => !paused || query.state.data === undefined,
    refetchOnWindowFocus: !paused,
    refetchOnReconnect: !paused,
  });
}

/**
 * Fetch an eligible artifact page, optionally filtered for progress discovery.
 *
 * Uses the GET /api/v1/executions/{execution_id}/artifacts endpoint.
 */
export function useExecutionArtifacts(
  executionId: number | undefined,
  isRunning = false,
  params: ArtifactPageParams & { type?: ArtifactType } = {},
  paused = false,
) {
  useArtifactReconciliation({
    scope: "execution",
    id: executionId,
    isRunning,
    paused,
  });
  return useQuery({
    queryKey: ["artifacts", "execution", executionId, params],
    queryFn: async ({ signal }) => {
      const response = await artifactRequest(
        __request<PaginatedArtifacts>(OpenAPI, {
          method: "GET",
          url: "/api/v1/executions/{execution_id}/artifacts",
          path: {
            execution_id: executionId!,
          },
          query: {
            cursor: params.cursor,
            per_page: params.perPage ?? 20,
            type: params.type,
          },
        }),
        signal,
      );
      return response;
    },
    enabled: (query) =>
      !!executionId && (!paused || query.state.data === undefined),
    refetchOnWindowFocus: !paused,
    refetchOnReconnect: !paused,
    staleTime: isRunning ? 3000 : 30000,
  });
}

/**
 * Fetch a single artifact by ID (includes data field for progress artifacts).
 *
 * @param isRunning - When true, polls every 3s for live updates. When false,
 *   uses a longer stale time and disables automatic polling.
 */
export function useArtifact(
  id: number | undefined,
  isRunning = false,
  paused = false,
) {
  useArtifactReconciliation({ scope: "detail", id, isRunning, paused });
  return useQuery({
    queryKey: ["artifacts", id],
    queryFn: async ({ signal }) => {
      const response = await artifactRequest(
        __request<{ data: ArtifactResponse }>(OpenAPI, {
          method: "GET",
          url: "/api/v1/artifacts/{id}",
          path: {
            id: id!,
          },
        }),
        signal,
      );
      return response;
    },
    enabled: (query) => !!id && (!paused || query.state.data === undefined),
    refetchOnWindowFocus: !paused,
    refetchOnReconnect: !paused,
    staleTime: isRunning ? 3000 : 30000,
  });
}

/**
 * Fetch versions for a given artifact ID.
 */
export function useArtifactVersions(
  artifactId: number | undefined,
  paused = false,
  params: ArtifactPageParams = {},
) {
  useArtifactReconciliation({ scope: "versions", id: artifactId, paused });
  return useQuery({
    queryKey: ["artifacts", artifactId, "versions", params],
    queryFn: async ({ signal }) => {
      const response = await artifactRequest(
        __request<PaginatedArtifactVersions>(OpenAPI, {
          method: "GET",
          url: "/api/v1/artifacts/{id}/versions",
          path: {
            id: artifactId!,
          },
          query: { cursor: params.cursor, per_page: params.perPage ?? 20 },
        }),
        signal,
      );

      return response;
    },
    enabled: (query) =>
      !!artifactId && (!paused || query.state.data === undefined),
    refetchOnWindowFocus: !paused,
    refetchOnReconnect: !paused,
    staleTime: 10000,
  });
}
