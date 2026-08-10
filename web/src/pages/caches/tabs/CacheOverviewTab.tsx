import { useState } from "react";
import { Link } from "react-router-dom";
import { ScrollText, Trash2 } from "lucide-react";
import { useAuth } from "@/contexts/AuthContext";
import { hasPermission } from "@/lib/permissions";
import { CacheGenerationState, type CacheNamespaceResponse } from "@/api";
import type { CacheOwnerParams } from "@/types/cache";
import {
  useCacheGenerations,
  useDeleteCacheNamespace,
} from "@/hooks/useCaches";
import CacheConfirmDialog from "@/components/caches/CacheConfirmDialog";
import {
  formatBytes,
  formatDateTime,
  formatFreshnessTarget,
  formatRecordCount,
  getCacheErrorMessage,
} from "@/components/caches/cacheUtils";

interface CacheOverviewTabProps {
  owner: CacheOwnerParams;
  namespace: CacheNamespaceResponse;
}

function PolicyRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between border-b border-gray-100 py-2 text-sm last:border-b-0">
      <span className="text-gray-500">{label}</span>
      <span className="font-medium text-gray-900">{value}</span>
    </div>
  );
}

export default function CacheOverviewTab({
  owner,
  namespace,
}: CacheOverviewTabProps) {
  const { user } = useAuth();
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const deleteNamespace = useDeleteCacheNamespace();
  const generationsQuery = useCacheGenerations(owner, namespace.namespace);

  const canDelete = hasPermission(user, "caches", "delete");

  const retainedGenerationCount = (
    generationsQuery.data?.data.generations ?? []
  ).filter(
    (generation) =>
      generation.status === CacheGenerationState.ACTIVE ||
      generation.status === CacheGenerationState.RETIRED,
  ).length;

  const recordsUsagePct =
    namespace.record_count !== null
      ? Math.min(
          100,
          Math.round(
            (namespace.record_count /
              Math.max(namespace.max_records_per_generation, 1)) *
              100,
          ),
        )
      : 0;

  const handleDelete = async () => {
    setDeleteError(null);
    try {
      await deleteNamespace.mutateAsync({
        owner,
        namespace: namespace.namespace,
      });
      setShowDeleteConfirm(false);
    } catch (err) {
      setDeleteError(getCacheErrorMessage(err, "Failed to delete namespace"));
    }
  };

  return (
    <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
      <div className="rounded-lg bg-white p-5 shadow">
        <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-gray-500">
          Namespace policy
        </h2>
        {namespace.managed && (
          <div className="mb-4 rounded-md border border-purple-200 bg-purple-50 px-3 py-2 text-sm text-purple-900">
            <p className="font-medium">Pack-managed policy (read-only)</p>
            <p className="mt-1 text-purple-800">
              Edit the cache definition in pack{" "}
              <span className="font-mono">
                {namespace.managing_pack_ref ?? "definition source"}
              </span>{" "}
              and reload the pack to apply policy changes.
            </p>
          </div>
        )}
        <PolicyRow
          label="Freshness target"
          value={formatFreshnessTarget(namespace.freshness_target_seconds)}
        />
        <PolicyRow
          label="Max records / generation"
          value={formatRecordCount(namespace.max_records_per_generation)}
        />
        <PolicyRow
          label="Max generation bytes"
          value={formatBytes(namespace.max_generation_bytes)}
        />
        <PolicyRow
          label="Max retained bytes"
          value={formatBytes(namespace.max_retained_bytes)}
        />
        <PolicyRow
          label="Max retained generations"
          value={String(namespace.max_retained_generations)}
        />
        <PolicyRow
          label="Max concurrent staging generations"
          value={String(namespace.max_staging_generations)}
        />
      </div>

      <div className="rounded-lg bg-white p-5 shadow">
        <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-gray-500">
          Active generation
        </h2>
        {!namespace.cache_not_populated ? (
          <>
            <div className="mb-3 flex items-center gap-2">
              <span className="font-mono text-sm text-gray-900">
                #{namespace.active_generation}
              </span>
              {namespace.stale && namespace.freshness_target_seconds > 0 && (
                <span className="inline-flex rounded-full bg-amber-100 px-2 py-0.5 text-xs font-semibold text-amber-800">
                  Stale
                </span>
              )}
            </div>
            <PolicyRow
              label="Records"
              value={formatRecordCount(namespace.record_count)}
            />
            <PolicyRow label="Size" value={formatBytes(namespace.size_bytes)} />
            <PolicyRow
              label="Source revision"
              value={namespace.source_revision ?? "—"}
            />
            <PolicyRow
              label="Last refreshed"
              value={formatDateTime(namespace.last_refreshed_at)}
            />
            <div className="mt-3">
              <div className="mb-1 flex justify-between text-xs text-gray-500">
                <span>Record quota usage</span>
                <span>{recordsUsagePct}%</span>
              </div>
              <div className="h-2 w-full rounded-full bg-gray-100">
                <div
                  className="h-2 rounded-full bg-teal-500"
                  style={{ width: `${recordsUsagePct}%` }}
                />
              </div>
            </div>
          </>
        ) : (
          <p className="text-sm text-gray-500">
            No generation has been published yet. This namespace is
            uninitialized — use the Refresh tab to publish the first generation.
          </p>
        )}
      </div>

      <div className="rounded-lg bg-white p-5 shadow lg:col-span-2">
        <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-gray-500">
          Definition provenance
        </h2>
        <PolicyRow
          label="Management"
          value={
            namespace.managed ? "Pack-managed (declarative)" : "API-managed"
          }
        />
        {namespace.managed && (
          <>
            <PolicyRow
              label="Managing pack"
              value={namespace.managing_pack_ref ?? "—"}
            />
            <PolicyRow
              label="Definition ref"
              value={namespace.definition_ref ?? "—"}
            />
          </>
        )}
      </div>

      <div className="rounded-lg bg-white p-5 shadow lg:col-span-2">
        <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-gray-500">
          Audit &amp; danger zone
        </h2>
        <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
          <Link
            to={`/audit-log?resource_type=cache_namespace&resource_ref=${encodeURIComponent(namespace.namespace)}`}
            className="inline-flex items-center gap-2 text-sm text-teal-700 hover:text-teal-900"
          >
            <ScrollText className="h-4 w-4" />
            View related audit events
          </Link>

          {namespace.managed ? (
            <p className="max-w-xl text-sm text-gray-600">
              Pack-managed namespaces cannot be deleted in the UI. Remove the
              cache definition from the managing pack and reload the pack.
            </p>
          ) : canDelete ? (
            <button
              onClick={() => setShowDeleteConfirm(true)}
              className="inline-flex items-center gap-2 rounded-md px-3 py-2 text-sm font-medium text-red-600 hover:bg-red-50"
            >
              <Trash2 className="h-4 w-4" />
              Delete namespace
            </button>
          ) : null}
        </div>
      </div>

      {showDeleteConfirm && (
        <CacheConfirmDialog
          title={`Delete namespace "${namespace.namespace}"?`}
          description="The namespace becomes immediately unreadable. Its generations and entries are not deleted instantly — they are tombstoned and reclaimed asynchronously by scheduled cleanup."
          tone="danger"
          confirmLabel="Delete namespace"
          confirmPhrase={namespace.namespace}
          isSubmitting={deleteNamespace.isPending}
          errorMessage={deleteError}
          impact={[
            {
              label: "Retained generations",
              value: String(retainedGenerationCount),
            },
            {
              label: "Active generation records",
              value: formatRecordCount(namespace.record_count),
            },
            {
              label: "Active generation size",
              value: formatBytes(namespace.size_bytes),
            },
          ]}
          onCancel={() => setShowDeleteConfirm(false)}
          onConfirm={handleDelete}
        />
      )}
    </div>
  );
}
