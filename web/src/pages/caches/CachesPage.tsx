import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Database, Plus, Search } from "lucide-react";
import { useAuth } from "@/contexts/AuthContext";
import { hasPermission } from "@/lib/permissions";
import { useCacheNamespaces } from "@/hooks/useCaches";
import { CacheNamespaceFreshness, OwnerType } from "@/api";
import ErrorDisplay from "@/components/common/ErrorDisplay";
import OwnerScopeSelector, {
  type OwnerScopeValue,
} from "@/components/caches/OwnerScopeSelector";
import {
  buildCacheNamespacePath,
  CacheNamespaceStatus,
  computeNamespaceStatus,
  formatBytes,
  formatDateTime,
  formatFreshnessTarget,
  formatRecordCount,
  getNamespaceStatusBadge,
  ownerRefForPath,
} from "@/components/caches/cacheUtils";
import CacheNamespaceCreateModal from "@/pages/caches/CacheNamespaceCreateModal";

const STATUS_FILTER_OPTIONS = [
  CacheNamespaceStatus.UNINITIALIZED,
  CacheNamespaceStatus.FRESH,
  CacheNamespaceStatus.STALE,
];
const NAMESPACE_PAGE_SIZE = 100;

// `GET /cache/namespaces` requires an owner scope — there is no cross-owner
// listing endpoint (see crates/api/src/routes/cache.rs::list_namespaces).
// System is a reasonable default because it needs no further input to
// become a valid, listable scope.
const DEFAULT_OWNER: OwnerScopeValue = {
  ownerType: OwnerType.SYSTEM,
  ownerRef: "",
};

function isOwnerReady(owner: OwnerScopeValue): boolean {
  if (
    owner.ownerType === OwnerType.SYSTEM ||
    owner.ownerType === OwnerType.IDENTITY
  ) {
    return true;
  }
  return owner.ownerRef.trim().length > 0;
}

function freshnessFilter(
  status: CacheNamespaceStatus | "",
): CacheNamespaceFreshness | undefined {
  switch (status) {
    case CacheNamespaceStatus.UNINITIALIZED:
      return CacheNamespaceFreshness.UNPOPULATED;
    case CacheNamespaceStatus.FRESH:
      return CacheNamespaceFreshness.FRESH;
    case CacheNamespaceStatus.STALE:
      return CacheNamespaceFreshness.STALE;
    default:
      return undefined;
  }
}

export default function CachesPage() {
  const navigate = useNavigate();
  const { user } = useAuth();
  const [owner, setOwner] = useState<OwnerScopeValue>(DEFAULT_OWNER);
  const [namespaceSearch, setNamespaceSearch] = useState("");
  const [status, setStatus] = useState<CacheNamespaceStatus | "">("");
  const [cursor, setCursor] = useState<string | undefined>();
  const [cursorHistory, setCursorHistory] = useState<Array<string | undefined>>(
    [],
  );
  const [showCreateModal, setShowCreateModal] = useState(false);

  const canCreate = hasPermission(user, "caches", "create");
  const ownerReady = isOwnerReady(owner);

  const { data, isLoading, error } = useCacheNamespaces(
    ownerReady
      ? { ownerType: owner.ownerType, ownerRef: owner.ownerRef }
      : undefined,
    {
      namespace: namespaceSearch.trim() || undefined,
      freshness: freshnessFilter(status),
      limit: NAMESPACE_PAGE_SIZE,
      cursor,
    },
  );

  const namespaces = data?.data.namespaces ?? [];
  const nextCursor = data?.data.next_cursor ?? null;

  const hasActiveFilters = Boolean(namespaceSearch.trim() || status);

  const resetPagination = () => {
    setCursor(undefined);
    setCursorHistory([]);
  };

  const clearFilters = () => {
    setNamespaceSearch("");
    setStatus("");
    resetPagination();
  };

  return (
    <div className="p-6 pb-28">
      <div className="mb-6 flex items-center justify-between">
        <div>
          <h1 className="flex items-center gap-3 text-3xl font-bold text-gray-900">
            <Database className="h-8 w-8 text-teal-600" />
            Data Caches
          </h1>
          <p className="mt-2 text-gray-600">
            Owner-scoped, generation-based caches of external business data.
            Separate from Keys &amp; Secrets — never used to store credentials.
          </p>
        </div>
        {canCreate && (
          <button
            onClick={() => setShowCreateModal(true)}
            className="flex items-center gap-2 rounded-lg bg-teal-600 px-4 py-2 text-white transition-colors hover:bg-teal-700"
          >
            <Plus className="h-4 w-4" />
            Create Namespace
          </button>
        )}
      </div>

      <div className="mb-6 rounded-lg bg-white p-4 shadow">
        <OwnerScopeSelector
          value={owner}
          onChange={(value) => {
            setOwner(value);
            resetPagination();
          }}
          ownerTypeLabelText="Owner scope to browse"
        />

        <div className="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              <div className="flex items-center gap-2">
                <Search className="h-4 w-4" />
                Namespace contains
              </div>
            </label>
            <input
              type="text"
              value={namespaceSearch}
              onChange={(event) => {
                setNamespaceSearch(event.target.value);
                resetPagination();
              }}
              placeholder="Filter on the server…"
              className="w-full rounded-lg border border-gray-300 px-3 py-2 focus:outline-none focus:ring-2 focus:ring-teal-500"
            />
          </div>

          <div>
            <label className="mb-1 block text-sm font-medium text-gray-700">
              Status
            </label>
            <select
              value={status}
              onChange={(event) => {
                setStatus(event.target.value as CacheNamespaceStatus | "");
                resetPagination();
              }}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 focus:outline-none focus:ring-2 focus:ring-teal-500"
            >
              <option value="">All statuses</option>
              {STATUS_FILTER_OPTIONS.map((value) => (
                <option key={value} value={value}>
                  {getNamespaceStatusBadge(value).label}
                </option>
              ))}
            </select>
          </div>
        </div>

        {hasActiveFilters && (
          <div className="mt-3 flex justify-end">
            <button
              onClick={clearFilters}
              className="text-sm text-gray-600 hover:text-gray-900"
            >
              Clear Filters
            </button>
          </div>
        )}
      </div>

      <div className="overflow-hidden rounded-lg bg-white shadow">
        {!ownerReady ? (
          <div className="p-12 text-center">
            <Database className="mx-auto h-12 w-12 text-gray-400" />
            <p className="mt-4 text-gray-600">
              Select an owner reference to list its cache namespaces
            </p>
          </div>
        ) : isLoading ? (
          <div className="p-12 text-center">
            <div className="mx-auto inline-block h-8 w-8 animate-spin rounded-full border-b-2 border-teal-600"></div>
            <p className="mt-4 text-gray-600">Loading cache namespaces...</p>
          </div>
        ) : error ? (
          <div className="p-8">
            <ErrorDisplay
              error={error}
              title="Failed to load cache namespaces"
            />
          </div>
        ) : namespaces.length === 0 ? (
          <div className="p-12 text-center">
            <Database className="mx-auto h-12 w-12 text-gray-400" />
            <p className="mt-4 text-gray-600">No cache namespaces found</p>
            <p className="mt-1 text-sm text-gray-500">
              {hasActiveFilters
                ? "Try adjusting your filters"
                : "Create a namespace to start publishing a cached dataset"}
            </p>
          </div>
        ) : (
          <>
            <div className="overflow-x-auto">
              <table className="min-w-full divide-y divide-gray-200">
                <thead className="bg-gray-50">
                  <tr>
                    <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Namespace
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Status
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Active generation
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Records / bytes
                    </th>
                    <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                      Last refresh / freshness target
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-200 bg-white">
                  {namespaces.map((namespace) => {
                    const namespaceStatus = computeNamespaceStatus(namespace);
                    const badge = getNamespaceStatusBadge(namespaceStatus);
                    return (
                      <tr
                        key={namespace.id}
                        className="cursor-pointer hover:bg-gray-50"
                        onClick={() =>
                          navigate(
                            buildCacheNamespacePath(
                              namespace.owner_type,
                              ownerRefForPath(namespace),
                              namespace.namespace,
                            ),
                          )
                        }
                      >
                        <td className="whitespace-nowrap px-6 py-4">
                          <div className="text-sm font-medium text-gray-900">
                            {namespace.namespace}
                          </div>
                        </td>
                        <td className="whitespace-nowrap px-6 py-4">
                          <span
                            className={`inline-flex rounded-full px-2 py-1 text-xs font-semibold leading-5 ${badge.classes}`}
                          >
                            {badge.label}
                          </span>
                        </td>
                        <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-900">
                          {namespace.active_generation ? (
                            <>
                              #{namespace.active_generation}
                              {namespace.source_revision && (
                                <div className="text-xs text-gray-500">
                                  rev: {namespace.source_revision}
                                </div>
                              )}
                            </>
                          ) : (
                            "—"
                          )}
                        </td>
                        <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-900">
                          {namespace.record_count !== null
                            ? `${formatRecordCount(namespace.record_count)} / ${formatBytes(namespace.size_bytes)}`
                            : "—"}
                        </td>
                        <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-900">
                          {formatDateTime(namespace.last_refreshed_at)}
                          <div className="text-xs text-gray-500">
                            target:{" "}
                            {formatFreshnessTarget(
                              namespace.freshness_target_seconds,
                            )}
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            <div className="flex items-center justify-between border-t border-gray-200 px-6 py-3">
              <button
                type="button"
                disabled={cursorHistory.length === 0 || isLoading}
                onClick={() => {
                  setCursorHistory((history) => {
                    const previous = history[history.length - 1];
                    setCursor(previous);
                    return history.slice(0, -1);
                  });
                }}
                className="rounded-md px-3 py-1.5 text-sm font-medium text-gray-600 hover:bg-gray-100 disabled:cursor-not-allowed disabled:text-gray-300"
              >
                Previous
              </button>
              <span className="text-xs text-gray-500">
                Up to {NAMESPACE_PAGE_SIZE} namespaces per page
              </span>
              <button
                type="button"
                disabled={!nextCursor || isLoading}
                onClick={() => {
                  if (!nextCursor) return;
                  setCursorHistory((history) => [...history, cursor]);
                  setCursor(nextCursor);
                }}
                className="rounded-md px-3 py-1.5 text-sm font-medium text-teal-700 hover:bg-teal-50 disabled:cursor-not-allowed disabled:text-gray-300"
              >
                Next
              </button>
            </div>
          </>
        )}
      </div>

      {showCreateModal && (
        <CacheNamespaceCreateModal
          onClose={() => setShowCreateModal(false)}
          onCreated={(created) => {
            setShowCreateModal(false);
            navigate(
              buildCacheNamespacePath(
                created.owner_type,
                ownerRefForPath(created),
                created.namespace,
              ),
            );
          }}
        />
      )}
    </div>
  );
}
