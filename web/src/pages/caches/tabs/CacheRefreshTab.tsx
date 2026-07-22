import { useMemo, useRef, useState } from "react";
import { AlertTriangle, Upload } from "lucide-react";
import {
  CacheGenerationState,
  type CacheEntryUpload,
  type CacheGenerationResponse,
  type CacheNamespaceResponse,
  type CacheOwnerParams,
  type JsonValue,
} from "@/api/cache";
import {
  useAbandonCacheGeneration,
  useBeginCacheRefresh,
  useCacheNamespace,
  usePromoteCacheGeneration,
  useSealCacheGeneration,
  useUploadCacheChunk,
} from "@/hooks/useCaches";
import CacheConfirmDialog from "@/components/caches/CacheConfirmDialog";
import {
  buildClientRefreshId,
  formatBytes,
  formatDateTime,
  formatRecordCount,
  getCacheErrorMessage,
  isPromotionConflictError,
  parseNdjsonRecordLine,
  streamFileRecordChunks,
} from "@/components/caches/cacheUtils";

interface CacheRefreshTabProps {
  owner: CacheOwnerParams;
  namespaceName: string;
  namespace: CacheNamespaceResponse;
}

// Bounded browser defaults. Comfortably under the server's
// MAX_INGEST_CHUNK_RECORDS (10,000) / MAX_INGEST_CHUNK_BYTES (32 MiB) so a
// slow connection still gets frequent progress updates and cheap retries.
const DEFAULT_RECORDS_PER_CHUNK = 500;
const DEFAULT_MAX_CHUNK_BYTES = 4 * 1024 * 1024;
const MAX_RECORDS_PER_CHUNK = 10_000;
// Leave room for the JSON array/owner wrapper under the API's 32 MiB raw-body
// limit even when many records are present.
const MAX_CHUNK_BYTES = 30 * 1024 * 1024;

type ChunkStatus = "pending" | "uploading" | "success" | "failed";
interface ChunkState {
  index: number;
  recordCount: number;
  status: ChunkStatus;
  error?: string;
}

export default function CacheRefreshTab({
  owner,
  namespaceName,
  namespace,
}: CacheRefreshTabProps) {
  const [file, setFile] = useState<File | null>(null);
  const [recordsPerChunk, setRecordsPerChunk] = useState(
    DEFAULT_RECORDS_PER_CHUNK,
  );
  const [maxChunkBytes, setMaxChunkBytes] = useState(DEFAULT_MAX_CHUNK_BYTES);
  const [sourceRevision, setSourceRevision] = useState("");
  const [totalRecords, setTotalRecords] = useState<number | null>(null);
  const [plannedChunkCount, setPlannedChunkCount] = useState<number | null>(
    null,
  );
  const [isCounting, setIsCounting] = useState(false);
  const [generation, setGeneration] = useState<CacheGenerationResponse | null>(
    null,
  );
  const [chunks, setChunks] = useState<ChunkState[]>([]);
  const [isUploading, setIsUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [promoteConflict, setPromoteConflict] = useState(false);
  const [showAbandonConfirm, setShowAbandonConfirm] = useState(false);
  const [genericError, setGenericError] = useState<string | null>(null);
  const cancelRef = useRef(false);

  const beginRefresh = useBeginCacheRefresh(owner, namespaceName);
  const uploadChunk = useUploadCacheChunk(owner, namespaceName);
  const sealGeneration = useSealCacheGeneration(owner, namespaceName);
  const promoteGeneration = usePromoteCacheGeneration(owner, namespaceName);
  const abandonGeneration = useAbandonCacheGeneration(owner, namespaceName);
  const namespaceQuery = useCacheNamespace(owner, namespaceName);

  const expectedChunkCount = plannedChunkCount;

  const uploadedRecords = useMemo(
    () =>
      chunks
        .filter((chunk) => chunk.status === "success")
        .reduce((sum, chunk) => sum + chunk.recordCount, 0),
    [chunks],
  );
  const allChunksUploaded =
    expectedChunkCount === 0 ||
    (chunks.length > 0 && chunks.every((chunk) => chunk.status === "success"));
  const hasFailedChunk = chunks.some((chunk) => chunk.status === "failed");

  const canPromote = generation?.status === CacheGenerationState.READY;
  const canAbandon =
    generation?.status === CacheGenerationState.STAGING ||
    generation?.status === CacheGenerationState.READY;

  const resetRefreshState = () => {
    setGeneration(null);
    setChunks([]);
    setTotalRecords(null);
    setPlannedChunkCount(null);
    setUploadError(null);
    setPromoteConflict(false);
  };

  const handleFileChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const selected = event.target.files?.[0] ?? null;
    setFile(selected);
    resetRefreshState();
  };

  const handleCount = async () => {
    if (!file) return;
    setIsCounting(true);
    setGenericError(null);
    try {
      let recordCount = 0;
      let chunkCount = 0;
      for await (const lines of streamFileRecordChunks(
        file,
        recordsPerChunk,
        maxChunkBytes,
      )) {
        recordCount += lines.length;
        chunkCount += 1;
      }
      setTotalRecords(recordCount);
      setPlannedChunkCount(chunkCount);
    } catch (err) {
      setGenericError(
        getCacheErrorMessage(err, "Failed to read the selected file"),
      );
    } finally {
      setIsCounting(false);
    }
  };

  const handleBegin = async () => {
    if (!file || expectedChunkCount === null) return;
    setGenericError(null);
    try {
      const clientRefreshId = buildClientRefreshId(
        `${owner.ownerType}:${owner.ownerRef ?? ""}:${namespaceName}:${recordsPerChunk}:${maxChunkBytes}`,
        file.name,
        file.size,
        file.lastModified,
      );
      const response = await beginRefresh.mutateAsync({
        client_refresh_id: clientRefreshId,
        expected_active_generation_id: namespace.active_generation ?? null,
        expected_chunk_count: expectedChunkCount,
        expected_record_count: totalRecords,
        source_revision: sourceRevision.trim() || undefined,
      });
      setGeneration(response.data);
      setChunks(
        Array.from({ length: expectedChunkCount }, (_, index) => ({
          index,
          recordCount: 0,
          status: "pending",
        })),
      );
    } catch (err) {
      setGenericError(getCacheErrorMessage(err, "Failed to begin refresh"));
    }
  };

  const handleUploadAll = async () => {
    if (!file || !generation) return;
    setIsUploading(true);
    setUploadError(null);
    cancelRef.current = false;

    try {
      let index = 0;
      let lineNumberOffset = 1;
      for await (const lines of streamFileRecordChunks(
        file,
        recordsPerChunk,
        maxChunkBytes,
      )) {
        const chunkIndex = index;
        index += 1;
        const baseLineNumber = lineNumberOffset;
        lineNumberOffset += lines.length;

        if (cancelRef.current) break;
        if (chunks[chunkIndex]?.status === "success") continue;

        setChunks((prev) =>
          prev.map((chunk) =>
            chunk.index === chunkIndex
              ? { ...chunk, status: "uploading", error: undefined }
              : chunk,
          ),
        );

        const parsed = lines.map((line, offset) =>
          parseNdjsonRecordLine(line, baseLineNumber + offset),
        );
        const invalid = parsed.find((result) => !result.ok);

        if (invalid && !invalid.ok) {
          setChunks((prev) =>
            prev.map((chunk) =>
              chunk.index === chunkIndex
                ? {
                    ...chunk,
                    status: "failed",
                    error: invalid.error,
                    recordCount: lines.length,
                  }
                : chunk,
            ),
          );
          setUploadError(
            `Chunk ${chunkIndex} failed validation: ${invalid.error}`,
          );
          break;
        }

        const entries: CacheEntryUpload[] = parsed.map((result) => {
          // `invalid` is checked above; every remaining result is `ok`.
          const { record } = result as Extract<typeof result, { ok: true }>;
          return {
            external_id: record.external_id,
            value: record.value as JsonValue,
            source_updated_at: record.source_updated_at,
            source_checksum: record.source_checksum,
          };
        });

        try {
          const response = await uploadChunk.mutateAsync({
            generationId: generation.generation_id,
            chunkIndex,
            entries,
          });
          // The upload response carries the generation's authoritative
          // running record/byte counts, so progress reflects server state
          // rather than only the client's local chunk bookkeeping.
          setGeneration(response.data);
          setChunks((prev) =>
            prev.map((chunk) =>
              chunk.index === chunkIndex
                ? { ...chunk, status: "success", recordCount: lines.length }
                : chunk,
            ),
          );
        } catch (err) {
          const message = getCacheErrorMessage(err, "Chunk upload failed");
          setChunks((prev) =>
            prev.map((chunk) =>
              chunk.index === chunkIndex
                ? {
                    ...chunk,
                    status: "failed",
                    error: message,
                    recordCount: lines.length,
                  }
                : chunk,
            ),
          );
          setUploadError(`Chunk ${chunkIndex} failed: ${message}`);
          break;
        }
      }
    } finally {
      setIsUploading(false);
    }
  };

  const handleSeal = async () => {
    if (!generation || expectedChunkCount === null) return;
    setGenericError(null);
    try {
      const sealed = await sealGeneration.mutateAsync({
        generationId: generation.generation_id,
        data: {
          expected_chunk_count: expectedChunkCount,
          expected_record_count: totalRecords,
        },
      });
      setGeneration(sealed.data);
    } catch (err) {
      setGenericError(getCacheErrorMessage(err, "Failed to seal generation"));
    }
  };

  const handlePromote = async () => {
    if (!generation) return;
    setGenericError(null);
    setPromoteConflict(false);
    try {
      const result = await promoteGeneration.mutateAsync({
        generationId: generation.generation_id,
        data: {
          expected_active_generation_id:
            generation.expected_active_generation_id,
        },
      });
      setGeneration(result.data);
    } catch (err) {
      if (isPromotionConflictError(err)) {
        setPromoteConflict(true);
        void namespaceQuery.refetch();
      } else {
        setGenericError(
          getCacheErrorMessage(err, "Failed to promote generation"),
        );
      }
    }
  };

  const handleAbandon = async () => {
    if (!generation) return;
    try {
      await abandonGeneration.mutateAsync(generation.generation_id);
      setShowAbandonConfirm(false);
      resetRefreshState();
      setFile(null);
    } catch (err) {
      setGenericError(getCacheErrorMessage(err, "Failed to abandon refresh"));
    }
  };

  return (
    <div className="space-y-6">
      <div className="rounded-lg border border-teal-200 bg-teal-50 p-4 text-sm text-teal-900">
        Browser ingestion is a controlled, manual workflow for smaller or ad-hoc
        refreshes. Scheduled 200,000-record synchronizations should run through
        an Attune action or the <code>attune cache refresh</code> CLI instead.
      </div>

      {genericError && (
        <p className="rounded-md bg-red-50 px-3 py-2 text-sm text-red-700">
          {genericError}
        </p>
      )}

      {!generation && (
        <div className="rounded-lg bg-white p-5 shadow">
          <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-gray-500">
            1. Select a local NDJSON file
          </h2>
          <input
            type="file"
            accept=".ndjson,.jsonl,.txt"
            onChange={handleFileChange}
            className="block text-sm text-gray-700"
          />
          {file && (
            <p className="mt-2 text-xs text-gray-500">
              {file.name} · {formatBytes(file.size)}
            </p>
          )}

          <div className="mt-4 grid grid-cols-1 gap-4 sm:grid-cols-3">
            <div>
              <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                Records per chunk
              </label>
              <input
                type="number"
                min={1}
                max={MAX_RECORDS_PER_CHUNK}
                value={recordsPerChunk}
                onChange={(event) =>
                  {
                    setRecordsPerChunk(
                    Math.min(
                      MAX_RECORDS_PER_CHUNK,
                      Math.max(1, Number(event.target.value) || 1),
                    ),
                    );
                    setTotalRecords(null);
                    setPlannedChunkCount(null);
                  }
                }
                className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                Max chunk size (KB)
              </label>
              <input
                type="number"
                min={1}
                max={MAX_CHUNK_BYTES / 1024}
                value={Math.round(maxChunkBytes / 1024)}
                onChange={(event) =>
                  {
                    setMaxChunkBytes(
                    Math.min(
                      MAX_CHUNK_BYTES,
                      Math.max(1, Number(event.target.value) || 1) * 1024,
                    ),
                    );
                    setTotalRecords(null);
                    setPlannedChunkCount(null);
                  }
                }
                className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
              />
            </div>
            <div>
              <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
                Source revision (optional)
              </label>
              <input
                type="text"
                value={sourceRevision}
                onChange={(event) => setSourceRevision(event.target.value)}
                placeholder="e.g. salesforce-export-2026-07-21"
                className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
              />
            </div>
          </div>

          <div className="mt-4 flex items-center gap-3">
            <button
              type="button"
              disabled={!file || isCounting}
              onClick={handleCount}
              className="rounded-md bg-gray-700 px-4 py-2 text-sm font-medium text-white hover:bg-gray-800 disabled:cursor-not-allowed disabled:bg-gray-300"
            >
              {isCounting ? "Scanning file…" : "Prepare (count records)"}
            </button>
            {totalRecords !== null && (
              <span className="text-sm text-gray-600">
                {formatRecordCount(totalRecords)} records · {expectedChunkCount}{" "}
                chunk{expectedChunkCount === 1 ? "" : "s"}
              </span>
            )}
          </div>

          {totalRecords !== null && (
            <div className="mt-4 border-t border-gray-100 pt-4">
              <button
                type="button"
                disabled={beginRefresh.isPending}
                onClick={handleBegin}
                className="rounded-md bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700 disabled:cursor-not-allowed disabled:bg-teal-300"
              >
                {beginRefresh.isPending
                  ? "Starting refresh…"
                  : "2. Begin refresh (create staging generation)"}
              </button>
            </div>
          )}
        </div>
      )}

      {generation && (
        <div className="rounded-lg bg-white p-5 shadow">
          <div className="mb-3 flex items-center justify-between">
            <h2 className="text-sm font-semibold uppercase tracking-wide text-gray-500">
              Staging generation #{generation.generation_id}
            </h2>
            <span className="rounded bg-blue-100 px-2 py-0.5 text-xs font-semibold text-blue-800">
              {generation.status}
            </span>
          </div>

          {generation.status === CacheGenerationState.STAGING &&
            generation.record_count > 0 &&
            chunks.every((chunk) => chunk.status === "pending") && (
              <p className="mb-3 rounded-md bg-blue-50 px-3 py-2 text-xs text-blue-800">
                This client refresh ID already has{" "}
                {formatRecordCount(generation.record_count)} record(s) uploaded
                from a previous session. Re-uploading the same file is safe —
                matching chunks are accepted as no-op replays.
              </p>
            )}

          <p className="mb-3 text-sm text-gray-600">
            {formatRecordCount(uploadedRecords)} /{" "}
            {formatRecordCount(totalRecords)} records uploaded across{" "}
            {chunks.filter((c) => c.status === "success").length} /{" "}
            {chunks.length} chunks.
          </p>
          <div className="mb-4 h-2 w-full rounded-full bg-gray-100">
            <div
              className="h-2 rounded-full bg-teal-500 transition-all"
              style={{
                width: `${
                  chunks.length === 0
                    ? 0
                    : (chunks.filter((c) => c.status === "success").length /
                        chunks.length) *
                      100
                }%`,
              }}
            />
          </div>

          {generation.status === CacheGenerationState.STAGING && (
            <div className="mb-4 flex items-center gap-3">
              <button
                type="button"
                disabled={isUploading || allChunksUploaded}
                onClick={handleUploadAll}
                className="inline-flex items-center gap-2 rounded-md bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700 disabled:cursor-not-allowed disabled:bg-teal-300"
              >
                <Upload className="h-4 w-4" />
                {isUploading
                  ? "Uploading…"
                  : allChunksUploaded
                    ? "All chunks uploaded"
                    : hasFailedChunk
                      ? "Retry remaining chunks"
                      : "3. Upload chunks"}
              </button>
              {isUploading && (
                <button
                  type="button"
                  onClick={() => {
                    cancelRef.current = true;
                  }}
                  className="text-sm text-gray-500 underline hover:text-gray-700"
                >
                  Pause after current chunk
                </button>
              )}
            </div>
          )}

          {uploadError && (
            <p className="mb-3 text-sm text-red-600">{uploadError}</p>
          )}

          <div className="max-h-56 overflow-y-auto rounded-md border border-gray-200">
            <table className="min-w-full divide-y divide-gray-200 text-xs">
              <thead className="sticky top-0 bg-gray-50">
                <tr>
                  <th className="px-2 py-1 text-left uppercase text-gray-500">
                    Chunk
                  </th>
                  <th className="px-2 py-1 text-left uppercase text-gray-500">
                    Status
                  </th>
                  <th className="px-2 py-1 text-left uppercase text-gray-500">
                    Detail
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-100">
                {chunks.map((chunk) => (
                  <tr key={chunk.index}>
                    <td className="px-2 py-1 font-mono">{chunk.index}</td>
                    <td className="px-2 py-1">
                      <span
                        className={
                          chunk.status === "success"
                            ? "text-green-700"
                            : chunk.status === "failed"
                              ? "text-red-700"
                              : chunk.status === "uploading"
                                ? "text-blue-700"
                                : "text-gray-400"
                        }
                      >
                        {chunk.status}
                      </span>
                    </td>
                    <td className="px-2 py-1 text-gray-500">
                      {chunk.status === "failed"
                        ? chunk.error
                        : chunk.status === "success"
                          ? `${chunk.recordCount} records`
                          : "—"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {generation.status === CacheGenerationState.STAGING && (
            <div className="mt-4 border-t border-gray-100 pt-4">
              <button
                type="button"
                disabled={!allChunksUploaded || sealGeneration.isPending}
                onClick={handleSeal}
                className="rounded-md bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700 disabled:cursor-not-allowed disabled:bg-teal-300"
              >
                {sealGeneration.isPending ? "Sealing…" : "4. Seal generation"}
              </button>
            </div>
          )}

          {generation.status === CacheGenerationState.READY && (
            <div className="mt-4 space-y-4 border-t border-gray-100 pt-4">
              <h3 className="text-sm font-semibold uppercase tracking-wide text-gray-500">
                5. Review before promotion
              </h3>
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <div className="rounded-md border border-gray-200 p-3">
                  <div className="mb-1 text-xs font-semibold uppercase text-gray-500">
                    Current active generation
                  </div>
                  {!namespace.cache_not_populated ? (
                    <ul className="space-y-1 text-sm text-gray-700">
                      <li>#{namespace.active_generation}</li>
                      <li>
                        {formatRecordCount(namespace.record_count)} records ·{" "}
                        {formatBytes(namespace.size_bytes)}
                      </li>
                      <li>
                        Last refreshed:{" "}
                        {formatDateTime(namespace.last_refreshed_at)}
                      </li>
                    </ul>
                  ) : (
                    <p className="text-sm text-gray-500">
                      None — this will be the first publication.
                    </p>
                  )}
                </div>
                <div className="rounded-md border border-teal-200 bg-teal-50 p-3">
                  <div className="mb-1 text-xs font-semibold uppercase text-teal-700">
                    New sealed generation
                  </div>
                  <ul className="space-y-1 text-sm text-teal-900">
                    <li>#{generation.generation_id} (ready)</li>
                    <li>
                      {formatRecordCount(generation.record_count)} records ·{" "}
                      {formatBytes(generation.size_bytes)}
                    </li>
                    <li>
                      Source revision: {generation.source_revision ?? "—"}
                    </li>
                  </ul>
                </div>
              </div>

              {promoteConflict && (
                <div className="rounded-md border border-amber-300 bg-amber-50 p-3">
                  <div className="flex items-center gap-2 text-sm font-medium text-amber-800">
                    <AlertTriangle className="h-4 w-4" />
                    Promotion conflict: the namespace's active generation
                    changed since this refresh began (now{" "}
                    {namespaceQuery.data?.data.active_generation ?? "unknown"}
                    ). Re-check the Generations tab before retrying.
                  </div>
                </div>
              )}

              <div className="flex flex-wrap items-center gap-3">
                <button
                  type="button"
                  disabled={!canPromote || promoteGeneration.isPending}
                  onClick={handlePromote}
                  className="rounded-md bg-teal-600 px-4 py-2 text-sm font-medium text-white hover:bg-teal-700 disabled:cursor-not-allowed disabled:bg-teal-300"
                >
                  {promoteGeneration.isPending
                    ? "Promoting…"
                    : `Promote (expects active = ${namespace.active_generation ?? "none"})`}
                </button>
                <button
                  type="button"
                  disabled
                  title="Force promotion requires a separate, strongly authorized operation that is not enabled in this release."
                  className="cursor-not-allowed rounded-md border-2 border-dashed border-red-300 px-4 py-2 text-sm font-medium text-red-400"
                >
                  Force promote (unavailable)
                </button>
              </div>
            </div>
          )}

          {generation.status === CacheGenerationState.ACTIVE && (
            <p className="mt-4 rounded-md bg-green-50 px-3 py-2 text-sm text-green-800">
              Promoted. This generation is now the namespace's active, readable
              snapshot.
            </p>
          )}

          {canAbandon && (
            <div className="mt-4 border-t border-gray-100 pt-4">
              <button
                type="button"
                onClick={() => setShowAbandonConfirm(true)}
                className="text-sm font-medium text-red-600 hover:text-red-800"
              >
                Abandon this refresh
              </button>
            </div>
          )}
        </div>
      )}

      {showAbandonConfirm && generation && (
        <CacheConfirmDialog
          title={`Abandon staging generation #${generation.generation_id}?`}
          description="The generation is marked failed and never becomes visible to readers. Its staged entries are reclaimed asynchronously. The server records a fixed audit reason for this action."
          tone="danger"
          confirmLabel="Abandon refresh"
          isSubmitting={abandonGeneration.isPending}
          impact={[
            {
              label: "Records uploaded",
              value: formatRecordCount(uploadedRecords),
            },
            {
              label: "Chunks accepted",
              value: `${chunks.filter((c) => c.status === "success").length} / ${chunks.length}`,
            },
          ]}
          onCancel={() => setShowAbandonConfirm(false)}
          onConfirm={handleAbandon}
        />
      )}
    </div>
  );
}
