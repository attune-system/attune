import { Fragment, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { CacheOwnerParams } from "@/types/cache";
import { useCacheGenerations } from "@/hooks/useCaches";
import ErrorDisplay from "@/components/common/ErrorDisplay";
import {
  formatBytes,
  formatDateTime,
  formatRecordCount,
  getGenerationStateBadge,
} from "@/components/caches/cacheUtils";

interface CacheGenerationsTabProps {
  owner: CacheOwnerParams;
  namespaceName: string;
}

const GENERATION_PAGE_SIZE = 100;

export default function CacheGenerationsTab({
  owner,
  namespaceName,
}: CacheGenerationsTabProps) {
  const [cursor, setCursor] = useState<string | undefined>();
  const [cursorHistory, setCursorHistory] = useState<Array<string | undefined>>(
    [],
  );
  const { data, isLoading, error } = useCacheGenerations(owner, namespaceName, {
    limit: GENERATION_PAGE_SIZE,
    cursor,
  });
  const [expandedId, setExpandedId] = useState<number | null>(null);

  if (isLoading) {
    return (
      <div className="p-12 text-center">
        <div className="mx-auto inline-block h-8 w-8 animate-spin rounded-full border-b-2 border-teal-600"></div>
        <p className="mt-4 text-gray-600">Loading generations…</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="rounded-lg bg-white p-5 shadow">
        <ErrorDisplay error={error} title="Failed to load generations" />
      </div>
    );
  }

  const generations = data?.data.generations ?? [];
  const nextCursor = data?.data.next_cursor ?? null;

  return (
    <div className="rounded-lg bg-white shadow">
      <div className="border-b border-gray-200 p-4">
        <h2 className="text-sm font-semibold uppercase tracking-wide text-gray-500">
          Generations
        </h2>
        <p className="mt-1 text-xs text-gray-500">
          Read-only lifecycle metadata. Generations are immutable — entries
          cannot be edited here.
        </p>
      </div>
      {generations.length === 0 ? (
        <p className="p-8 text-center text-sm text-gray-500">
          No generations yet. Use the Refresh tab to publish the first one.
        </p>
      ) : (
        <>
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-200">
              <thead className="bg-gray-50">
                <tr>
                  <th className="w-8 px-3 py-2"></th>
                  <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                    ID
                  </th>
                  <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                    State
                  </th>
                  <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                    Records / bytes
                  </th>
                  <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                    Source revision
                  </th>
                  <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                    Created
                  </th>
                  <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                    Sealed / Activated / Retired
                  </th>
                  <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                    Readable until
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-100">
                {generations.map((generation) => {
                  const badge = getGenerationStateBadge(generation.status);
                  const isExpanded = expandedId === generation.generation_id;
                  return (
                    <Fragment key={generation.generation_id}>
                      <tr
                        className="cursor-pointer hover:bg-gray-50"
                        onClick={() =>
                          setExpandedId(
                            isExpanded ? null : generation.generation_id,
                          )
                        }
                      >
                        <td className="px-3 py-2 text-gray-400">
                          {isExpanded ? (
                            <ChevronDown className="h-4 w-4" />
                          ) : (
                            <ChevronRight className="h-4 w-4" />
                          )}
                        </td>
                        <td className="px-3 py-2 font-mono text-sm">
                          #{generation.generation_id}
                        </td>
                        <td className="px-3 py-2">
                          <span
                            className={`inline-flex rounded-full px-2 py-0.5 text-xs font-semibold ${badge.classes}`}
                          >
                            {badge.label}
                          </span>
                        </td>
                        <td className="px-3 py-2 text-sm text-gray-900">
                          {formatRecordCount(generation.record_count)} /{" "}
                          {formatBytes(generation.size_bytes)}
                        </td>
                        <td className="px-3 py-2 text-sm text-gray-900">
                          {generation.source_revision ?? "—"}
                        </td>
                        <td className="px-3 py-2 text-sm text-gray-500">
                          {formatDateTime(generation.created)}
                        </td>
                        <td className="px-3 py-2 text-xs text-gray-500">
                          {formatDateTime(generation.sealed)} /{" "}
                          {formatDateTime(generation.activated)} /{" "}
                          {formatDateTime(generation.retired)}
                        </td>
                        <td className="px-3 py-2 text-xs text-gray-500">
                          {formatDateTime(generation.readable_until)}
                        </td>
                      </tr>
                      {isExpanded && (
                        <tr>
                          <td colSpan={8} className="bg-gray-50 p-4">
                            <dl className="grid grid-cols-2 gap-3 text-sm sm:grid-cols-4">
                              <div>
                                <dt className="text-xs uppercase tracking-wide text-gray-500">
                                  Client refresh ID
                                </dt>
                                <dd className="font-mono text-gray-900">
                                  {generation.client_refresh_id}
                                </dd>
                              </div>
                              <div>
                                <dt className="text-xs uppercase tracking-wide text-gray-500">
                                  Expected chunk count
                                </dt>
                                <dd className="text-gray-900">
                                  {generation.expected_chunk_count}
                                </dd>
                              </div>
                              <div>
                                <dt className="text-xs uppercase tracking-wide text-gray-500">
                                  Expected record count
                                </dt>
                                <dd className="text-gray-900">
                                  {formatRecordCount(
                                    generation.expected_record_count,
                                  )}
                                </dd>
                              </div>
                              <div>
                                <dt className="text-xs uppercase tracking-wide text-gray-500">
                                  Failed
                                </dt>
                                <dd className="text-gray-900">
                                  {formatDateTime(generation.failed)}
                                </dd>
                              </div>
                            </dl>
                            {generation.failure_reason && (
                              <p className="mt-3 rounded-md bg-red-50 px-3 py-2 text-sm text-red-700">
                                Failure reason: {generation.failure_reason}
                              </p>
                            )}
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  );
                })}
              </tbody>
            </table>
          </div>
          <div className="flex items-center justify-between border-t border-gray-200 px-4 py-3">
            <button
              type="button"
              disabled={cursorHistory.length === 0 || isLoading}
              onClick={() => {
                setExpandedId(null);
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
              Up to {GENERATION_PAGE_SIZE} generations per page
            </span>
            <button
              type="button"
              disabled={!nextCursor || isLoading}
              onClick={() => {
                if (!nextCursor) return;
                setExpandedId(null);
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
  );
}
