import { useState, useCallback, useMemo, useEffect, memo } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Search, X, Eye, EyeOff, Download, Package } from "lucide-react";
import LiveStreamControl, {
  DEFAULT_LIVE_LIST_MAX_ITEMS,
} from "@/components/common/LiveStreamControl";
import Pagination from "@/components/executions/Pagination";
import {
  useArtifactsList,
  type ArtifactSummary,
  type ArtifactType,
  type ArtifactVisibility,
  type OwnerType,
} from "@/hooks/useArtifacts";
import { useArtifactStream } from "@/hooks/useArtifactStream";
import { useArtifactLivePage } from "@/hooks/useArtifactLivePage";
import {
  TYPE_OPTIONS,
  VISIBILITY_OPTIONS,
  SCOPE_OPTIONS,
  getArtifactTypeIcon,
  getArtifactTypeBadge,
  getScopeBadge,
  formatBytes,
  formatDate,
  formatTime,
  downloadArtifact,
  isDownloadable,
} from "./artifactHelpers";

// ============================================================================
// Results Table (memoized so filter typing doesn't re-render rows)
// ============================================================================

const ArtifactsResultsTable = memo(
  ({
    artifacts,
    isLoading,
    isFetching,
    error,
    hasActiveFilters,
    clearFilters,
  }: {
    artifacts: ArtifactSummary[];
    isLoading: boolean;
    isFetching: boolean;
    error: Error | null;
    hasActiveFilters: boolean;
    clearFilters: () => void;
  }) => {
    if (isLoading && artifacts.length === 0) {
      return (
        <div className="bg-white shadow rounded-lg">
          <div className="flex items-center justify-center h-64">
            <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
            <p className="ml-4 text-gray-600">Loading artifacts...</p>
          </div>
        </div>
      );
    }

    if (error && artifacts.length === 0) {
      return (
        <div className="bg-white shadow rounded-lg p-12 text-center">
          <p className="text-red-600">Failed to load artifacts</p>
          <p className="text-sm text-gray-600 mt-2">{error.message}</p>
        </div>
      );
    }

    if (artifacts.length === 0) {
      return (
        <div className="bg-white shadow rounded-lg p-12 text-center">
          <Package className="mx-auto h-12 w-12 text-gray-400" />
          <p className="mt-4 text-gray-600">No artifacts found</p>
          <p className="text-sm text-gray-500 mt-1">
            {hasActiveFilters
              ? "Try adjusting your filters"
              : "Artifacts will appear here when executions produce output"}
          </p>
          {hasActiveFilters && (
            <button
              onClick={clearFilters}
              className="mt-3 text-sm text-blue-600 hover:text-blue-800"
            >
              Clear filters
            </button>
          )}
        </div>
      );
    }

    return (
      <div className="relative">
        <div className="mb-2 h-4 text-xs text-gray-500">
          {isFetching && (
            <div className="flex items-center gap-2">
              <div className="h-3 w-3 animate-spin rounded-full border-b-2 border-blue-600" />
              <span>Refreshing artifacts…</span>
            </div>
          )}
        </div>

        {error && (
          <div className="mb-4 bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
            <p>Error refreshing: {error.message}</p>
          </div>
        )}

        <div className="bg-white shadow rounded-lg overflow-hidden">
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-200">
              <thead className="bg-gray-50">
                <tr>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Artifact
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Type
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Visibility
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Scope / Owner
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Size
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Created
                  </th>
                  <th className="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Actions
                  </th>
                </tr>
              </thead>
              <tbody className="bg-white divide-y divide-gray-200">
                {artifacts.map((artifact) => {
                  const typeBadge = getArtifactTypeBadge(artifact.type);
                  const scopeBadge = getScopeBadge(artifact.scope);
                  return (
                    <tr key={artifact.id} className="hover:bg-gray-50">
                      <td className="px-6 py-4">
                        <div className="flex items-center gap-2">
                          {getArtifactTypeIcon(artifact.type)}
                          <div className="min-w-0">
                            <Link
                              to={`/artifacts/${artifact.id}`}
                              className="text-sm font-medium text-blue-600 hover:text-blue-800 truncate block"
                            >
                              {artifact.name || artifact.ref}
                            </Link>
                            {artifact.name && (
                              <div className="text-xs text-gray-500 font-mono truncate">
                                {artifact.ref}
                              </div>
                            )}
                          </div>
                        </div>
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap">
                        <span
                          className={`px-2 py-1 inline-flex text-xs leading-5 font-semibold rounded-full ${typeBadge.classes}`}
                        >
                          {typeBadge.label}
                        </span>
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap">
                        <div className="flex items-center gap-1.5 text-sm">
                          {artifact.visibility === "public" ? (
                            <>
                              <Eye className="h-3.5 w-3.5 text-green-600" />
                              <span className="text-green-700">Public</span>
                            </>
                          ) : (
                            <>
                              <EyeOff className="h-3.5 w-3.5 text-gray-400" />
                              <span className="text-gray-600">Private</span>
                            </>
                          )}
                        </div>
                      </td>
                      <td className="px-6 py-4">
                        <div>
                          <span
                            className={`px-2 py-0.5 inline-flex text-xs leading-5 font-semibold rounded-full ${scopeBadge.classes}`}
                          >
                            {scopeBadge.label}
                          </span>
                          {artifact.owner && (
                            <div className="text-xs text-gray-500 mt-0.5 font-mono truncate max-w-[160px]">
                              {artifact.owner}
                            </div>
                          )}
                        </div>
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-700">
                        {formatBytes(artifact.size_bytes)}
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap">
                        <div className="text-sm text-gray-900">
                          {formatTime(artifact.created)}
                        </div>
                        <div className="text-xs text-gray-500">
                          {formatDate(artifact.created)}
                        </div>
                      </td>
                      <td className="px-6 py-4 whitespace-nowrap text-right">
                        <div className="flex items-center justify-end gap-2">
                          <Link
                            to={`/artifacts/${artifact.id}`}
                            className="text-gray-500 hover:text-blue-600"
                            title="View details"
                          >
                            <Eye className="h-4 w-4" />
                          </Link>
                          {isDownloadable(artifact.type) && (
                            <button
                              onClick={() =>
                                downloadArtifact(artifact.id, artifact.ref)
                              }
                              className="text-gray-500 hover:text-blue-600"
                              title="Download latest version"
                            >
                              <Download className="h-4 w-4" />
                            </button>
                          )}
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      </div>
    );
  },
);

ArtifactsResultsTable.displayName = "ArtifactsResultsTable";

// ============================================================================
// Main Page
// ============================================================================

export default function ArtifactsPage() {
  const [searchParams] = useSearchParams();

  const [page, setPage] = useState(1);
  const pageSize = 20;

  const [nameFilter, setNameFilter] = useState(searchParams.get("name") || "");
  const [typeFilter, setTypeFilter] = useState<ArtifactType | "">(
    (searchParams.get("type") as ArtifactType) || "",
  );
  const [visibilityFilter, setVisibilityFilter] = useState<
    ArtifactVisibility | ""
  >((searchParams.get("visibility") as ArtifactVisibility) || "");
  const [scopeFilter, setScopeFilter] = useState<OwnerType | "">(
    (searchParams.get("scope") as OwnerType) || "",
  );
  const [ownerFilter, setOwnerFilter] = useState(
    searchParams.get("owner") || "",
  );
  const [executionFilter, setExecutionFilter] = useState(
    searchParams.get("execution") || "",
  );

  // Debounce text inputs
  const [debouncedName, setDebouncedName] = useState(nameFilter);
  const [debouncedOwner, setDebouncedOwner] = useState(ownerFilter);
  const [debouncedExecution, setDebouncedExecution] = useState(executionFilter);

  useEffect(() => {
    const t = setTimeout(() => setDebouncedName(nameFilter), 400);
    return () => clearTimeout(t);
  }, [nameFilter]);

  useEffect(() => {
    const t = setTimeout(() => setDebouncedOwner(ownerFilter), 400);
    return () => clearTimeout(t);
  }, [ownerFilter]);

  useEffect(() => {
    const t = setTimeout(() => setDebouncedExecution(executionFilter), 400);
    return () => clearTimeout(t);
  }, [executionFilter]);

  // Build query params
  const queryParams = useMemo(() => {
    const params: Record<string, unknown> = { page, perPage: pageSize };
    if (debouncedName) params.name = debouncedName;
    if (typeFilter) params.type = typeFilter;
    if (visibilityFilter) params.visibility = visibilityFilter;
    if (scopeFilter) params.scope = scopeFilter;
    if (debouncedOwner) params.owner = debouncedOwner;
    if (debouncedExecution) {
      const n = Number(debouncedExecution);
      if (!isNaN(n)) params.execution = n;
    }
    return params;
  }, [
    page,
    pageSize,
    debouncedName,
    typeFilter,
    visibilityFilter,
    scopeFilter,
    debouncedOwner,
    debouncedExecution,
  ]);

  const [livePaused, setLivePaused] = useState(false);
  useArtifactLivePage({ scope: "list", page, setPage, paused: livePaused });

  const { data, isLoading, isFetching, error } = useArtifactsList(
    queryParams,
    livePaused,
  );

  // Subscribe to real-time artifact updates
  const { isConnected } = useArtifactStream({
    enabled: true,
    paused: livePaused,
  });

  const artifacts = useMemo(() => data?.items || [], [data]);
  const total = data?.pagination?.total_items || 0;

  const hasActiveFilters =
    !!nameFilter ||
    !!typeFilter ||
    !!visibilityFilter ||
    !!scopeFilter ||
    !!ownerFilter ||
    !!executionFilter;

  const clearFilters = useCallback(() => {
    setNameFilter("");
    setTypeFilter("");
    setVisibilityFilter("");
    setScopeFilter("");
    setOwnerFilter("");
    setExecutionFilter("");
    setPage(1);
  }, []);

  return (
    <div className="p-6 pb-28">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-3xl font-bold text-gray-900">Artifacts</h1>
            <p className="mt-2 text-gray-600">
              Files, progress indicators, and data produced by executions
            </p>
          </div>
          <LiveStreamControl
            paused={livePaused}
            onTogglePaused={() => setLivePaused((paused) => !paused)}
            connected={isConnected}
            maxItems={DEFAULT_LIVE_LIST_MAX_ITEMS}
            itemLabel="artifacts"
            showRetentionHint={false}
          />
        </div>
      </div>

      {/* Filters */}
      <p className="mb-3 text-sm text-gray-500">
        Live refreshes return to page one. Pause updates to browse older pages;
        resuming starts from page one.
      </p>
      <div className="bg-white shadow rounded-lg p-4 mb-6">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            <Search className="h-5 w-5 text-gray-400" />
            <h2 className="text-lg font-semibold">Filter Artifacts</h2>
          </div>
          {hasActiveFilters && (
            <button
              onClick={clearFilters}
              className="flex items-center gap-1 text-sm text-gray-600 hover:text-gray-900"
            >
              <X className="h-4 w-4" />
              Clear Filters
            </button>
          )}
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 lg:grid-cols-6 gap-4">
          {/* Name search */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Name
            </label>
            <input
              type="text"
              value={nameFilter}
              onChange={(e) => {
                setNameFilter(e.target.value);
                setPage(1);
              }}
              placeholder="Search by name..."
              className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
            />
          </div>

          {/* Type */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Type
            </label>
            <select
              value={typeFilter}
              onChange={(e) => {
                setTypeFilter(e.target.value as ArtifactType | "");
                setPage(1);
              }}
              className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
            >
              <option value="">All Types</option>
              {TYPE_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>

          {/* Visibility */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Visibility
            </label>
            <select
              value={visibilityFilter}
              onChange={(e) => {
                setVisibilityFilter(e.target.value as ArtifactVisibility | "");
                setPage(1);
              }}
              className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
            >
              <option value="">All</option>
              {VISIBILITY_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>

          {/* Scope */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Scope
            </label>
            <select
              value={scopeFilter}
              onChange={(e) => {
                setScopeFilter(e.target.value as OwnerType | "");
                setPage(1);
              }}
              className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
            >
              <option value="">All Scopes</option>
              {SCOPE_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>

          {/* Owner */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Owner
            </label>
            <input
              type="text"
              value={ownerFilter}
              onChange={(e) => {
                setOwnerFilter(e.target.value);
                setPage(1);
              }}
              placeholder="e.g. mypack.deploy"
              className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
            />
          </div>

          {/* Execution ID */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Execution
            </label>
            <input
              type="text"
              value={executionFilter}
              onChange={(e) => {
                setExecutionFilter(e.target.value);
                setPage(1);
              }}
              placeholder="Execution ID"
              className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
            />
          </div>
        </div>

        {data && (
          <div className="mt-3 text-sm text-gray-600">
            Showing {artifacts.length} of {total} artifacts
            {hasActiveFilters && " (filtered)"}
          </div>
        )}
      </div>

      {/* Results */}
      <ArtifactsResultsTable
        artifacts={artifacts}
        isLoading={isLoading}
        isFetching={isFetching}
        error={error as Error | null}
        hasActiveFilters={hasActiveFilters}
        clearFilters={clearFilters}
      />

      <Pagination
        page={page}
        setPage={setPage}
        pageSize={pageSize}
        itemCount={artifacts.length}
        total={total}
        itemLabel="artifacts"
        floating
      />
    </div>
  );
}
