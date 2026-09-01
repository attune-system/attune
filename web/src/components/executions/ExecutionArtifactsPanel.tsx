import { useState, useMemo, useEffect, useCallback, useRef } from "react";
import { formatDistanceToNow } from "date-fns";
import {
  ChevronDown,
  ChevronRight,
  FileText,
  FileImage,
  File,
  BarChart3,
  Link as LinkIcon,
  Table2,
  Package,
  Loader2,
  Download,
  Eye,
  X,
  Radio,
} from "lucide-react";
import {
  useExecutionArtifacts,
  useArtifact,
  type ArtifactSummary,
  type ArtifactType,
} from "@/hooks/useArtifacts";
import { useArtifactStream } from "@/hooks/useArtifactStream";
import { formatJsonValue } from "@/lib/format-utils";
import { OpenAPI } from "@/api/core/OpenAPI";

interface ExecutionArtifactsPanelProps {
  executionId: number;
  /** Whether the execution is still running (enables polling) */
  isRunning?: boolean;
}

function getArtifactTypeIcon(type: ArtifactType) {
  switch (type) {
    case "file_text":
      return <FileText className="h-4 w-4 text-blue-500" />;
    case "file_image":
      return <FileImage className="h-4 w-4 text-purple-500" />;
    case "file_binary":
      return <File className="h-4 w-4 text-gray-500" />;
    case "file_datatable":
      return <Table2 className="h-4 w-4 text-green-500" />;
    case "progress":
      return <BarChart3 className="h-4 w-4 text-amber-500" />;
    case "url":
      return <LinkIcon className="h-4 w-4 text-cyan-500" />;
    case "other":
    default:
      return <Package className="h-4 w-4 text-gray-400" />;
  }
}

function getArtifactTypeBadge(type: ArtifactType): {
  label: string;
  classes: string;
} {
  switch (type) {
    case "file_text":
      return { label: "Text File", classes: "bg-blue-100 text-blue-800" };
    case "file_image":
      return { label: "Image", classes: "bg-purple-100 text-purple-800" };
    case "file_binary":
      return { label: "Binary", classes: "bg-gray-100 text-gray-800" };
    case "file_datatable":
      return { label: "Data Table", classes: "bg-green-100 text-green-800" };
    case "progress":
      return { label: "Progress", classes: "bg-amber-100 text-amber-800" };
    case "url":
      return { label: "URL", classes: "bg-cyan-100 text-cyan-800" };
    case "other":
    default:
      return { label: "Other", classes: "bg-gray-100 text-gray-700" };
  }
}

function formatBytes(bytes: number | null): string {
  if (bytes == null || bytes === 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Download the latest version of an artifact using a fetch with auth token. */
async function downloadArtifact(artifactId: number, artifactRef: string) {
  const token = localStorage.getItem("access_token");
  const url = `${OpenAPI.BASE}/api/v1/artifacts/${artifactId}/download`;

  const response = await fetch(url, {
    headers: {
      Authorization: `Bearer ${token}`,
    },
  });

  if (!response.ok) {
    console.error(`Download failed: ${response.status} ${response.statusText}`);
    return;
  }

  // Extract filename from Content-Disposition header or fall back to ref
  const disposition = response.headers.get("Content-Disposition");
  let filename = artifactRef.replace(/\./g, "_") + ".bin";
  if (disposition) {
    const match = disposition.match(/filename="?([^"]+)"?/);
    if (match) filename = match[1];
  }

  const blob = await response.blob();
  const blobUrl = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = blobUrl;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(blobUrl);
}

// ============================================================================
// Text File Artifact Detail
// ============================================================================

interface TextFileDetailProps {
  artifactId: number;
  artifactName: string | null;
  isRunning?: boolean;
  onClose: () => void;
}

function TextFileDetail({
  artifactId,
  artifactName,
  isRunning = false,
  onClose,
}: TextFileDetailProps) {
  const [content, setContent] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [isLoadingContent, setIsLoadingContent] = useState(true);
  const [isStreaming, setIsStreaming] = useState(false);
  const [isWaiting, setIsWaiting] = useState(false);
  const [streamDone, setStreamDone] = useState(false);
  const preRef = useRef<HTMLPreElement>(null);
  const streamAbortRef = useRef<AbortController | null>(null);
  // Track whether the user has scrolled away from the bottom so we can
  // auto-scroll only when they're already at the end.
  const userScrolledAwayRef = useRef(false);

  // Auto-scroll the <pre> to the bottom when new content arrives,
  // unless the user has deliberately scrolled up.
  const scrollToBottom = useCallback(() => {
    const el = preRef.current;
    if (el && !userScrolledAwayRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, []);

  // Detect whether the user has scrolled away from the bottom.
  const handleScroll = useCallback(() => {
    const el = preRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    userScrolledAwayRef.current = !atBottom;
  }, []);

  // ---- Streaming path (used when execution is running) ----
  /* eslint-disable react-hooks/set-state-in-effect -- subscribes to an external SSE stream and records its lifecycle */
  useEffect(() => {
    if (!isRunning) return;

    const token = localStorage.getItem("access_token");
    if (!token) {
      queueMicrotask(() => {
        setLoadError("No authentication token available");
        setIsLoadingContent(false);
      });
      return;
    }

    const controller = new AbortController();
    streamAbortRef.current = controller;
    setIsStreaming(true);
    setStreamDone(false);

    const handleStreamEvent = (eventName: string, data: string) => {
      switch (eventName) {
        case "waiting":
          setIsWaiting(true);
          setIsLoadingContent(false);
          if (data.includes("File found")) {
            setIsWaiting(false);
          }
          break;
        case "content":
          setContent(data);
          setLoadError(null);
          setIsLoadingContent(false);
          setIsWaiting(false);
          requestAnimationFrame(scrollToBottom);
          break;
        case "append":
          setContent((prev) => (prev ?? "") + data);
          setLoadError(null);
          requestAnimationFrame(scrollToBottom);
          break;
        case "done":
          setStreamDone(true);
          setIsStreaming(false);
          setIsWaiting(false);
          controller.abort();
          break;
        case "error":
          if (data) {
            setLoadError(data);
          }
          break;
      }
    };

    const consumeSseBlock = (block: string) => {
      let eventName = "message";
      const dataLines: string[] = [];
      for (const line of block.split(/\r?\n/)) {
        if (line.startsWith("event:")) {
          eventName = line.slice("event:".length).trim();
        } else if (line.startsWith("data:")) {
          dataLines.push(line.slice("data:".length).trimStart());
        }
      }
      handleStreamEvent(eventName, dataLines.join("\n"));
    };

    void (async () => {
      try {
        const response = await fetch(
          `${OpenAPI.BASE}/api/v1/artifacts/${artifactId}/stream`,
          {
            headers: { Authorization: `Bearer ${token}` },
            signal: controller.signal,
          },
        );
        if (!response.ok || !response.body) {
          throw new Error(`Stream failed with status ${response.status}`);
        }

        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        while (!controller.signal.aborted) {
          const { value, done } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          let separatorIndex = buffer.search(/\r?\n\r?\n/);
          while (separatorIndex >= 0) {
            const block = buffer.slice(0, separatorIndex);
            buffer = buffer.slice(
              buffer[separatorIndex] === "\r"
                ? separatorIndex + 4
                : separatorIndex + 2,
            );
            if (block.trim()) {
              consumeSseBlock(block);
            }
            separatorIndex = buffer.search(/\r?\n\r?\n/);
          }
        }
      } catch (err) {
        if (!controller.signal.aborted) {
          setLoadError(err instanceof Error ? err.message : "Stream failed");
        }
      } finally {
        setIsStreaming(false);
      }
    })();

    return () => {
      controller.abort();
      streamAbortRef.current = null;
      setIsStreaming(false);
      setIsWaiting(false);
    };
  }, [artifactId, isRunning, scrollToBottom]);
  /* eslint-enable react-hooks/set-state-in-effect */

  // ---- Fetch fallback (used when not running, or SSE never connected) ----
  const fetchContent = useCallback(async () => {
    const token = localStorage.getItem("access_token");
    const url = `${OpenAPI.BASE}/api/v1/artifacts/${artifactId}/download`;
    try {
      const response = await fetch(url, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!response.ok) {
        setLoadError(`HTTP ${response.status}: ${response.statusText}`);
        setIsLoadingContent(false);
        return;
      }
      const text = await response.text();
      setContent(text);
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : "Unknown error");
    } finally {
      setIsLoadingContent(false);
      setIsWaiting(false);
    }
  }, [artifactId]);

  // When NOT running (execution completed), use download endpoint to get final content.
  // This also handles the transition from running → completed (stream was aborted,
  // now fetch the full file).
  useEffect(() => {
    if (isRunning) return;
    queueMicrotask(() => void fetchContent());
  }, [isRunning, fetchContent]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      onClick={onClose}
    >
      <div
        className="bg-white rounded-lg shadow-xl w-full max-w-3xl max-h-[80vh] flex flex-col m-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-gray-200">
          <h3 className="text-base font-semibold text-gray-900 flex items-center gap-2">
            <FileText className="h-4 w-4 text-blue-500" />
            {artifactName ?? "Text File"}
          </h3>
          <div className="flex items-center gap-2">
            {isStreaming && !streamDone && (
              <div className="flex items-center gap-1 text-xs text-green-600">
                <Radio className="h-3 w-3 animate-pulse" />
                <span>Streaming</span>
              </div>
            )}
            {streamDone && (
              <span className="text-xs text-gray-500">Stream complete</span>
            )}
            {isWaiting && (
              <div className="flex items-center gap-1 text-xs text-amber-600">
                <Loader2 className="h-3 w-3 animate-spin" />
                <span>Waiting for file…</span>
              </div>
            )}
            <button
              onClick={onClose}
              className="text-gray-400 hover:text-gray-600 p-1 rounded"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-hidden p-4">
          {isLoadingContent && !isWaiting && (
            <div className="flex items-center gap-2 py-2 text-sm text-gray-500">
              <Loader2 className="h-4 w-4 animate-spin" />
              Loading content…
            </div>
          )}

          {loadError && (
            <p className="text-xs text-red-600 italic">Error: {loadError}</p>
          )}

          {!isLoadingContent && !loadError && content !== null && (
            <pre
              ref={preRef}
              onScroll={handleScroll}
              className="h-full max-h-[60vh] overflow-y-auto bg-gray-900 text-gray-100 rounded p-3 text-xs font-mono whitespace-pre-wrap break-all"
            >
              {content || <span className="text-gray-500 italic">(empty)</span>}
            </pre>
          )}

          {isWaiting && content === null && !loadError && (
            <div className="bg-gray-900 rounded p-3 text-xs text-gray-500 italic">
              Waiting for the worker to write the file…
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ============================================================================
// Progress Artifact Detail
// ============================================================================

interface ProgressDetailProps {
  artifactId: number;
  isRunning?: boolean;
  onClose: () => void;
}

function ProgressDetail({
  artifactId,
  isRunning = false,
  onClose,
}: ProgressDetailProps) {
  const { data: artifactData, isLoading } = useArtifact(artifactId, isRunning);
  const artifact = artifactData?.data;

  const progressEntries = useMemo(() => {
    if (!artifact?.data || !Array.isArray(artifact.data)) return [];
    return artifact.data as Array<Record<string, unknown>>;
  }, [artifact]);

  const latestEntry =
    progressEntries.length > 0
      ? progressEntries[progressEntries.length - 1]
      : null;
  const latestPercent =
    latestEntry && typeof latestEntry.percent === "number"
      ? latestEntry.percent
      : null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      onClick={onClose}
    >
      <div
        className="bg-white rounded-lg shadow-xl w-full max-w-2xl max-h-[80vh] flex flex-col m-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-gray-200">
          <h3 className="text-base font-semibold text-gray-900 flex items-center gap-2">
            <BarChart3 className="h-4 w-4 text-amber-500" />
            {artifact?.name ?? "Progress"}
          </h3>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 p-1 rounded"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="p-5 overflow-y-auto">
          {isLoading && (
            <div className="flex items-center gap-2 py-2 text-sm text-gray-500">
              <Loader2 className="h-4 w-4 animate-spin" />
              Loading progress…
            </div>
          )}

          {!isLoading && latestPercent != null && (
            <div className="mb-4">
              <div className="flex items-center justify-between text-xs text-gray-600 mb-1">
                <span>
                  {latestEntry?.message
                    ? formatJsonValue(latestEntry.message)
                    : `${latestPercent}%`}
                </span>
                <span className="font-mono">{latestPercent}%</span>
              </div>
              <div className="w-full bg-gray-200 rounded-full h-2.5">
                <div
                  className="bg-amber-500 h-2.5 rounded-full transition-all duration-300"
                  style={{ width: `${Math.min(latestPercent, 100)}%` }}
                />
              </div>
            </div>
          )}

          {!isLoading && progressEntries.length > 0 && (
            <div className="max-h-64 overflow-y-auto">
              <table className="w-full text-xs">
                <thead>
                  <tr className="text-left text-gray-500 border-b border-gray-200">
                    <th className="pb-1 pr-2">#</th>
                    <th className="pb-1 pr-2">%</th>
                    <th className="pb-1 pr-2">Message</th>
                    <th className="pb-1">Time</th>
                  </tr>
                </thead>
                <tbody>
                  {progressEntries.map((entry, idx) => (
                    <tr
                      key={idx}
                      className="border-b border-gray-100 last:border-0"
                    >
                      <td className="py-1 pr-2 text-gray-400 font-mono">
                        {typeof entry.iteration === "number"
                          ? entry.iteration
                          : idx + 1}
                      </td>
                      <td className="py-1 pr-2 font-mono">
                        {typeof entry.percent === "number"
                          ? `${entry.percent}%`
                          : "—"}
                      </td>
                      <td className="py-1 pr-2 text-gray-700 truncate max-w-[200px]">
                        {entry.message ? formatJsonValue(entry.message) : "—"}
                      </td>
                      <td className="py-1 text-gray-400 whitespace-nowrap">
                        {entry.timestamp
                          ? formatDistanceToNow(
                              new Date(String(entry.timestamp)),
                              { addSuffix: true },
                            )
                          : "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {!isLoading && progressEntries.length === 0 && (
            <p className="text-xs text-gray-500 italic">
              No progress entries yet.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

// ============================================================================
// Main Panel
// ============================================================================

export default function ExecutionArtifactsPanel({
  executionId,
  isRunning = false,
}: ExecutionArtifactsPanelProps) {
  const [expandedId, setExpandedId] = useState<number | null>(null);
  const [previewProgressId, setPreviewProgressId] = useState<number | null>(
    null,
  );
  const [previewTextFileId, setPreviewTextFileId] = useState<number | null>(
    null,
  );

  // Subscribe to real-time artifact notifications for this execution.
  // WebSocket-driven cache invalidation replaces most of the polling need,
  // but we keep polling as a fallback (staleTime/refetchInterval in the hook).
  useArtifactStream({ executionId, enabled: isRunning });

  const { data, isLoading, error } = useExecutionArtifacts(
    executionId,
    isRunning,
  );

  const artifacts: ArtifactSummary[] = useMemo(() => {
    return data?.data ?? [];
  }, [data]);

  const summary = useMemo(() => {
    const total = artifacts.length;
    const files = artifacts.filter((a) =>
      ["file_text", "file_binary", "file_image", "file_datatable"].includes(
        a.type,
      ),
    ).length;
    const progress = artifacts.filter((a) => a.type === "progress").length;
    const other = total - files - progress;
    return { total, files, progress, other };
  }, [artifacts]);

  // Don't render anything if there are no artifacts and we're not loading
  if (!isLoading && artifacts.length === 0 && !error) {
    return null;
  }

  return (
    <div className="bg-white shadow rounded-lg">
      {/* Header */}
      <div className="flex items-center justify-between p-4">
        <div className="flex items-center gap-2">
          <Package className="h-4 w-4 text-indigo-500" />
          <h2 className="text-lg font-semibold">Artifacts</h2>
          {!isLoading && (
            <span className="text-xs text-gray-500">({summary.total})</span>
          )}
          {isRunning && (
            <div className="flex items-center gap-1.5 text-xs text-blue-600">
              <Loader2 className="h-3 w-3 animate-spin" />
              <span>Live</span>
            </div>
          )}
        </div>
      </div>

      {/* Content */}
      <div className="px-4 pb-4">
        {isLoading && (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-5 w-5 animate-spin text-gray-400" />
            <span className="ml-2 text-sm text-gray-500">
              Loading artifacts…
            </span>
          </div>
        )}

        {error && (
          <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded text-sm">
            Error loading artifacts:{" "}
            {error instanceof Error ? error.message : "Unknown error"}
          </div>
        )}

        {!isLoading && !error && artifacts.length > 0 && (
          <div className="divide-y divide-gray-100">
            {artifacts.map((artifact) => {
              const badge = getArtifactTypeBadge(artifact.type);
              const isProgress = artifact.type === "progress";
              const isTextFile = artifact.type === "file_text";
              const isFile = [
                "file_text",
                "file_binary",
                "file_image",
                "file_datatable",
              ].includes(artifact.type);
              const isExpanded = expandedId === artifact.id;

              return (
                <div key={artifact.id}>
                  {/* Compact row: icon + name + type badge */}
                  <button
                    className="w-full flex items-center gap-2 px-3 py-2 text-left hover:bg-gray-50 transition-colors rounded"
                    onClick={() =>
                      setExpandedId(isExpanded ? null : artifact.id)
                    }
                  >
                    {isExpanded ? (
                      <ChevronDown className="h-3.5 w-3.5 text-gray-400 flex-shrink-0" />
                    ) : (
                      <ChevronRight className="h-3.5 w-3.5 text-gray-400 flex-shrink-0" />
                    )}
                    {getArtifactTypeIcon(artifact.type)}
                    <span
                      className="text-sm text-gray-900 truncate flex-1 min-w-0"
                      title={artifact.name ?? artifact.ref}
                    >
                      {artifact.name ?? artifact.ref}
                    </span>
                    <span
                      className={`inline-flex px-1.5 py-0.5 rounded text-[10px] font-medium flex-shrink-0 ${badge.classes}`}
                    >
                      {badge.label}
                    </span>
                  </button>

                  {/* Expanded detail dropdown */}
                  {isExpanded && (
                    <div className="px-3 pb-3 ml-9 space-y-2">
                      <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
                        <dt className="text-gray-500">Ref</dt>
                        <dd
                          className="font-mono text-gray-700 truncate"
                          title={artifact.ref}
                        >
                          {artifact.ref}
                        </dd>
                        {artifact.size_bytes != null &&
                          artifact.size_bytes > 0 && (
                            <>
                              <dt className="text-gray-500">Size</dt>
                              <dd className="text-gray-700">
                                {formatBytes(artifact.size_bytes)}
                              </dd>
                            </>
                          )}
                        <dt className="text-gray-500">Created</dt>
                        <dd className="text-gray-700">
                          {formatDistanceToNow(new Date(artifact.created), {
                            addSuffix: true,
                          })}
                        </dd>
                      </dl>
                      <div className="flex items-center gap-1 pt-1">
                        {(isProgress || isTextFile) && (
                          <button
                            onClick={() => {
                              if (isProgress) setPreviewProgressId(artifact.id);
                              else setPreviewTextFileId(artifact.id);
                            }}
                            className="inline-flex items-center gap-1 px-2 py-1 text-xs rounded hover:bg-gray-100 text-gray-600 hover:text-blue-600"
                          >
                            <Eye className="h-3.5 w-3.5" />
                            Preview
                          </button>
                        )}
                        {isFile && (
                          <button
                            onClick={() =>
                              downloadArtifact(artifact.id, artifact.ref)
                            }
                            className="inline-flex items-center gap-1 px-2 py-1 text-xs rounded hover:bg-gray-100 text-gray-600 hover:text-blue-600"
                          >
                            <Download className="h-3.5 w-3.5" />
                            Download
                          </button>
                        )}
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {/* Preview modals */}
        {previewProgressId != null && (
          <ProgressDetail
            artifactId={previewProgressId}
            isRunning={isRunning}
            onClose={() => setPreviewProgressId(null)}
          />
        )}
        {previewTextFileId != null && (
          <TextFileDetail
            artifactId={previewTextFileId}
            artifactName={
              artifacts.find((a) => a.id === previewTextFileId)?.name ?? null
            }
            isRunning={isRunning}
            onClose={() => setPreviewTextFileId(null)}
          />
        )}
      </div>
    </div>
  );
}
