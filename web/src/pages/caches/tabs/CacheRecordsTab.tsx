import { useState } from "react";
import { AlertTriangle, Search } from "lucide-react";
import type { CacheEntryResponse } from "@/api";
import type { CacheOwnerParams } from "@/types/cache";
import {
  useCacheEntriesGetMany,
  useCacheEntryLookup,
  useCacheEntryScan,
  useResetCurrentCacheEntryScans,
} from "@/hooks/useCaches";
import {
  DEFAULT_SCAN_PAGE_SIZE,
  formatBytes,
  formatDateTime,
  formatJsonPreview,
  getCacheErrorMessage,
  isCacheNotPopulatedError,
  isSnapshotExpiredError,
  MAX_MULTI_LOOKUP_IDS,
  parseExternalIdsInput,
} from "@/components/caches/cacheUtils";

interface CacheRecordsTabProps {
  owner: CacheOwnerParams;
  namespaceName: string;
}

function EntryValueCell({ entry }: { entry: CacheEntryResponse }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div>
      <button
        type="button"
        onClick={() => setExpanded((prev) => !prev)}
        className="font-mono text-xs text-teal-700 hover:text-teal-900"
      >
        {expanded ? "Hide value" : "Show value"}
      </button>
      {expanded ? (
        <pre className="mt-1 max-w-md overflow-x-auto rounded bg-gray-50 p-2 text-xs">
          {JSON.stringify(entry.value, null, 2)}
        </pre>
      ) : (
        <div className="max-w-md truncate font-mono text-xs text-gray-400">
          {formatJsonPreview(entry.value)}
        </div>
      )}
    </div>
  );
}

export default function CacheRecordsTab({
  owner,
  namespaceName,
}: CacheRecordsTabProps) {
  // ── Exact lookup ─────────────────────────────────────────────────────
  const [lookupId, setLookupId] = useState("");
  const lookup = useCacheEntryLookup(owner, namespaceName);

  // ── Bounded multi-ID lookup ──────────────────────────────────────────
  const [multiInput, setMultiInput] = useState("");
  const getMany = useCacheEntriesGetMany(owner, namespaceName);

  // ── Cursor-page browsing ─────────────────────────────────────────────
  const [browsing, setBrowsing] = useState(false);
  const [pageSize, setPageSize] = useState(DEFAULT_SCAN_PAGE_SIZE);
  const [requireFresh, setRequireFresh] = useState(false);
  const [pinnedGeneration, setPinnedGeneration] = useState<number | undefined>(
    undefined,
  );
  const [cursor, setCursor] = useState<string | undefined>(undefined);
  const resetCurrentScans = useResetCurrentCacheEntryScans(
    owner,
    namespaceName,
  );

  const scan = useCacheEntryScan(owner, namespaceName, {
    generationId: pinnedGeneration,
    cursor,
    limit: pageSize,
    requireFresh,
    enabled: browsing,
  });

  const scanExpired = isSnapshotExpiredError(scan.error);

  const resolveCurrentGeneration = () => {
    void resetCurrentScans();
    setPinnedGeneration(undefined);
    setCursor(undefined);
  };

  const startBrowsing = () => {
    resolveCurrentGeneration();
    setBrowsing(true);
  };

  const nextPage = () => {
    const data = scan.data?.data;
    if (!data?.next_cursor) return;
    setPinnedGeneration(data.generation_id);
    setCursor(data.next_cursor);
  };

  /** Discards the expired pin and resolves the namespace's current generation. */
  const restartOnCurrentGeneration = resolveCurrentGeneration;

  /** Abandons the pinned generation entirely and re-resolves the current active one. */
  const startNewBrowse = resolveCurrentGeneration;

  const { ids: parsedMultiIds, exceededLimit } =
    parseExternalIdsInput(multiInput);

  return (
    <div className="space-y-6">
      <div className="rounded-lg bg-white p-5 shadow">
        <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-gray-500">
          Exact lookup
        </h2>
        <form
          className="flex gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (lookupId.trim()) lookup.mutate(lookupId.trim());
          }}
        >
          <input
            value={lookupId}
            onChange={(event) => setLookupId(event.target.value)}
            placeholder="External ID"
            className="flex-1 rounded-md border border-gray-300 px-3 py-2 font-mono text-sm"
          />
          <button
            type="submit"
            disabled={!lookupId.trim() || lookup.isPending}
            className="inline-flex items-center gap-2 rounded-md bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700 disabled:cursor-not-allowed disabled:bg-teal-300"
          >
            <Search className="h-4 w-4" />
            {lookup.isPending ? "Looking up…" : "Look up"}
          </button>
        </form>

        {lookup.isError && (
          <p className="mt-3 text-sm text-red-600">
            {isCacheNotPopulatedError(lookup.error)
              ? "This namespace has never had a generation published (cache_not_populated)."
              : getCacheErrorMessage(lookup.error, "Lookup failed")}
          </p>
        )}

        {lookup.isSuccess && (
          <div className="mt-4 rounded-md border border-gray-200 p-3">
            <div className="mb-2 flex items-center gap-2 text-xs text-gray-500">
              <span>Generation #{lookup.data.data.generation_id}</span>
              {lookup.data.data.stale && (
                <span className="rounded bg-amber-100 px-1.5 py-0.5 text-amber-800">
                  stale
                </span>
              )}
            </div>
            {lookup.data.data.item ? (
              <div className="space-y-2 text-sm">
                <div className="flex justify-between">
                  <span className="text-gray-500">Source updated</span>
                  <span>
                    {formatDateTime(lookup.data.data.item.source_updated_at)}
                  </span>
                </div>
                <div className="flex justify-between">
                  <span className="text-gray-500">Size</span>
                  <span>{formatBytes(lookup.data.data.item.size_bytes)}</span>
                </div>
                <EntryValueCell entry={lookup.data.data.item} />
              </div>
            ) : (
              <p className="text-sm text-gray-500">
                No active entry for this external ID.
              </p>
            )}
          </div>
        )}
      </div>

      <div className="rounded-lg bg-white p-5 shadow">
        <h2 className="mb-1 text-sm font-semibold uppercase tracking-wide text-gray-500">
          Bounded multi-ID lookup
        </h2>
        <p className="mb-3 text-xs text-gray-500">
          One external ID per line (commas also accepted). Bounded to{" "}
          {MAX_MULTI_LOOKUP_IDS.toLocaleString()} IDs per request.
        </p>
        <textarea
          value={multiInput}
          onChange={(event) => setMultiInput(event.target.value)}
          rows={4}
          placeholder={"sfdc-001\nsfdc-002\nsfdc-003"}
          className="w-full rounded-md border border-gray-300 px-3 py-2 font-mono text-sm"
        />
        <div className="mt-2 flex items-center justify-between">
          <span className="text-xs text-gray-500">
            {parsedMultiIds.length.toLocaleString()} unique ID
            {parsedMultiIds.length === 1 ? "" : "s"} parsed
            {exceededLimit && (
              <span className="ml-2 text-amber-700">
                (truncated to {MAX_MULTI_LOOKUP_IDS.toLocaleString()})
              </span>
            )}
          </span>
          <button
            type="button"
            disabled={parsedMultiIds.length === 0 || getMany.isPending}
            onClick={() => getMany.mutate(parsedMultiIds)}
            className="rounded-md bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700 disabled:cursor-not-allowed disabled:bg-teal-300"
          >
            {getMany.isPending ? "Fetching…" : "Fetch records"}
          </button>
        </div>

        {getMany.isError && (
          <p className="mt-3 text-sm text-red-600">
            {getCacheErrorMessage(getMany.error, "Multi-ID lookup failed")}
          </p>
        )}

        {getMany.isSuccess && (
          <div className="mt-4">
            <div className="mb-2 text-xs text-gray-500">
              Generation #{getMany.data.data.generation_id} ·{" "}
              {getMany.data.data.items.length} found ·{" "}
              {getMany.data.data.missing_external_ids.length} missing
            </div>
            <div className="max-h-96 overflow-y-auto rounded-md border border-gray-200">
              <table className="min-w-full divide-y divide-gray-200 text-sm">
                <thead className="bg-gray-50">
                  <tr>
                    <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                      External ID
                    </th>
                    <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                      Value
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-100">
                  {getMany.data.data.items.map((entry) => (
                    <tr key={entry.external_id}>
                      <td className="px-3 py-2 font-mono">
                        {entry.external_id}
                      </td>
                      <td className="px-3 py-2">
                        <EntryValueCell entry={entry} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {getMany.data.data.missing_external_ids.length > 0 && (
              <p className="mt-2 text-xs text-gray-500">
                Missing: {getMany.data.data.missing_external_ids.join(", ")}
              </p>
            )}
          </div>
        )}
      </div>

      <div className="rounded-lg bg-white p-5 shadow">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold uppercase tracking-wide text-gray-500">
            Cursor-page browsing
          </h2>
          <div className="flex items-center gap-3 text-xs text-gray-500">
            <label className="flex items-center gap-1">
              <input
                type="checkbox"
                checked={requireFresh}
                onChange={(event) => {
                  setRequireFresh(event.target.checked);
                  resolveCurrentGeneration();
                }}
              />
              Require fresh
            </label>
            <label className="flex items-center gap-1">
              Page size
              <select
                value={pageSize}
                onChange={(event) => {
                  setPageSize(Number(event.target.value));
                  resolveCurrentGeneration();
                }}
                className="rounded border border-gray-300 px-1 py-0.5"
              >
                {[25, 50, 100, 250].map((size) => (
                  <option key={size} value={size}>
                    {size}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </div>

        <p className="mb-3 text-xs text-gray-500">
          Browses one generation-pinned snapshot at a time. There is no offset
          paging and no "load all" action — advance one bounded page at a time.
        </p>

        {!browsing ? (
          <button
            type="button"
            onClick={startBrowsing}
            className="rounded-md bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700"
          >
            Start browsing
          </button>
        ) : (
          <>
            {scan.data?.data && (
              <div className="mb-3 flex flex-wrap items-center gap-2 text-xs text-gray-500">
                <span>Pinned generation #{scan.data.data.generation_id}</span>
                {scan.data.data.stale && (
                  <span className="rounded bg-amber-100 px-1.5 py-0.5 text-amber-800">
                    stale
                  </span>
                )}
                {scan.data.data.cursor_expires_at && (
                  <span>
                    Cursor expires:{" "}
                    {formatDateTime(scan.data.data.cursor_expires_at)}
                  </span>
                )}
              </div>
            )}

            {scan.isLoading && (
              <p className="text-sm text-gray-500">Loading page…</p>
            )}

            {scan.isError && scanExpired && (
              <div className="mb-3 rounded-md border border-amber-300 bg-amber-50 p-3">
                <div className="flex items-center gap-2 text-sm font-medium text-amber-800">
                  <AlertTriangle className="h-4 w-4" />
                  This browsing snapshot has expired.
                </div>
                <div className="mt-2 flex gap-3">
                  <button
                    type="button"
                    onClick={restartOnCurrentGeneration}
                    className="rounded-md bg-amber-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-amber-700"
                  >
                    Restart on current generation
                  </button>
                  <button
                    type="button"
                    onClick={startNewBrowse}
                    className="text-xs font-medium text-gray-600 underline hover:text-gray-800"
                  >
                    Start a new browse (latest generation)
                  </button>
                </div>
              </div>
            )}

            {scan.isError && !scanExpired && (
              <p className="text-sm text-red-600">
                {isCacheNotPopulatedError(scan.error)
                  ? "This namespace has never had a generation published (cache_not_populated)."
                  : getCacheErrorMessage(scan.error, "Failed to load page")}
              </p>
            )}

            {scan.data?.data && scan.data.data.items.length === 0 && (
              <p className="text-sm text-gray-500">
                No records in this generation.
              </p>
            )}

            {scan.data?.data && scan.data.data.items.length > 0 && (
              <div className="max-h-[28rem] overflow-y-auto rounded-md border border-gray-200">
                <table className="min-w-full divide-y divide-gray-200 text-sm">
                  <thead className="sticky top-0 bg-gray-50">
                    <tr>
                      <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                        External ID
                      </th>
                      <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                        Source updated
                      </th>
                      <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                        Size
                      </th>
                      <th className="px-3 py-2 text-left text-xs font-medium uppercase text-gray-500">
                        Value
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-gray-100">
                    {scan.data.data.items.map((entry) => (
                      <tr key={entry.external_id}>
                        <td className="px-3 py-2 font-mono">
                          {entry.external_id}
                        </td>
                        <td className="px-3 py-2 text-gray-500">
                          {formatDateTime(entry.source_updated_at)}
                        </td>
                        <td className="px-3 py-2 text-gray-500">
                          {formatBytes(entry.size_bytes)}
                        </td>
                        <td className="px-3 py-2">
                          <EntryValueCell entry={entry} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            <div className="mt-3 flex gap-3">
              <button
                type="button"
                disabled={!scan.data?.data?.next_cursor || scan.isFetching}
                onClick={nextPage}
                className="rounded-md bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700 disabled:cursor-not-allowed disabled:bg-teal-300"
              >
                Next page
              </button>
              <button
                type="button"
                onClick={startNewBrowse}
                className="rounded-md px-4 py-2 text-sm font-medium text-gray-600 hover:bg-gray-100"
              >
                Restart browsing
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
