import { useState, useMemo, useCallback, useEffect } from "react";
import LiveStreamControl from "@/components/common/LiveStreamControl";
import { useParams, Link, useSearchParams } from "react-router-dom";
import {
  ArrowLeft,
  Download,
  Eye,
  EyeOff,
  Loader2,
  FileText,
  Clock,
  Hash,
  X,
} from "lucide-react";
import {
  useArtifact,
  useArtifactVersions,
  type ArtifactResponse,
  type ArtifactVersionSummary,
} from "@/hooks/useArtifacts";
import { useArtifactStream } from "@/hooks/useArtifactStream";
import { useArtifactCursorPage } from "@/hooks/useArtifactCursorPage";
import ArtifactCursorControls from "@/components/common/ArtifactCursorControls";
import { formatJsonValue } from "@/lib/format-utils";
import {
  ARTIFACT_PREVIEW_BYTES,
  readArtifactPreview,
} from "@/lib/artifact-preview";
import { OpenAPI } from "@/api/core/OpenAPI";
import {
  getArtifactTypeIcon,
  getArtifactTypeBadge,
  getScopeBadge,
  formatBytes,
  formatDate,
  downloadArtifact,
  isDownloadable,
} from "./artifactHelpers";

// ============================================================================
// Text content viewer
// ============================================================================

function TextContentViewer({
  artifactId,
  versionNumber,
  versionId,
  label,
}: {
  artifactId: number;
  versionNumber?: number;
  versionId?: number;
  label: string;
}) {
  // Track a fetch key so that when deps change we re-derive initial state
  // instead of calling setState synchronously inside useEffect.
  const fetchKey = `${artifactId}:${versionNumber ?? "latest"}:${versionId ?? ""}`;
  const [settledKey, setSettledKey] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [truncated, setTruncated] = useState(false);

  const isLoading = settledKey !== fetchKey;

  useEffect(() => {
    const controller = new AbortController();

    const token = localStorage.getItem("access_token");
    const url = versionNumber
      ? `${OpenAPI.BASE}/api/v1/artifacts/${artifactId}/versions/${versionNumber}/download?version_id=${versionId}`
      : `${OpenAPI.BASE}/api/v1/artifacts/${artifactId}/download`;

    fetch(url, {
      headers: {
        Authorization: `Bearer ${token}`,
        Range: `bytes=0-${ARTIFACT_PREVIEW_BYTES - 1}`,
      },
      signal: controller.signal,
    })
      .then(async (response) => {
        const preview = await readArtifactPreview(response);
        if (controller.signal.aborted) return;
        setContent(preview.text);
        setTruncated(preview.truncated);
        setLoadError(null);
      })
      .catch(() => {
        if (!controller.signal.aborted) {
          setLoadError("Content unavailable or access denied");
          setContent(null);
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setSettledKey(fetchKey);
      });

    return () => {
      controller.abort();
    };
  }, [artifactId, versionNumber, versionId, fetchKey]);

  if (isLoading) {
    return (
      <div className="flex items-center gap-2 py-4 text-sm text-gray-500">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading {label}...
      </div>
    );
  }

  if (loadError) {
    return <div className="py-4 text-sm text-red-600">{loadError}</div>;
  }

  return (
    <div>
      {truncated && (
        <p className="mb-2 text-sm text-gray-600">
          Preview limited to the first 64 KiB. Download the file to read the
          full content.
        </p>
      )}
      <pre className="max-h-96 overflow-y-auto bg-gray-900 text-gray-100 rounded-lg p-4 text-xs font-mono whitespace-pre-wrap break-all">
        {content || <span className="text-gray-500 italic">(empty)</span>}
      </pre>
    </div>
  );
}

// ============================================================================
// Progress viewer
// ============================================================================

function ProgressViewer({ data }: { data: unknown }) {
  const entries = useMemo(() => {
    if (!data || !Array.isArray(data)) return [];
    return data as Array<Record<string, unknown>>;
  }, [data]);

  const latestEntry = entries.length > 0 ? entries[entries.length - 1] : null;
  const latestPercent =
    latestEntry && typeof latestEntry.percent === "number"
      ? latestEntry.percent
      : null;

  if (entries.length === 0) {
    return (
      <p className="text-sm text-gray-500 italic">No progress entries yet.</p>
    );
  }

  return (
    <div>
      {latestPercent != null && (
        <div className="mb-4">
          <div className="flex items-center justify-between text-sm text-gray-600 mb-1">
            <span>
              {latestEntry?.message
                ? formatJsonValue(latestEntry.message)
                : `${latestPercent}%`}
            </span>
            <span className="font-mono">{latestPercent}%</span>
          </div>
          <div className="w-full bg-gray-200 rounded-full h-3">
            <div
              className="bg-amber-500 h-3 rounded-full transition-all duration-300"
              style={{ width: `${Math.min(latestPercent, 100)}%` }}
            />
          </div>
        </div>
      )}

      <div className="max-h-64 overflow-y-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="text-left text-gray-500 border-b border-gray-200">
              <th className="pb-2 pr-3">#</th>
              <th className="pb-2 pr-3">%</th>
              <th className="pb-2 pr-3">Message</th>
              <th className="pb-2">Time</th>
            </tr>
          </thead>
          <tbody>
            {entries.map((entry, idx) => (
              <tr key={idx} className="border-b border-gray-100 last:border-0">
                <td className="py-1.5 pr-3 text-gray-400 font-mono">
                  {typeof entry.iteration === "number"
                    ? entry.iteration
                    : idx + 1}
                </td>
                <td className="py-1.5 pr-3 font-mono">
                  {typeof entry.percent === "number"
                    ? `${entry.percent}%`
                    : "\u2014"}
                </td>
                <td className="py-1.5 pr-3 text-gray-700 truncate max-w-[300px]">
                  {entry.message ? formatJsonValue(entry.message) : "\u2014"}
                </td>
                <td className="py-1.5 text-gray-400 whitespace-nowrap">
                  {entry.timestamp
                    ? new Date(String(entry.timestamp)).toLocaleTimeString()
                    : "\u2014"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// ============================================================================
// Version row
// ============================================================================

function VersionRow({
  version,
  artifactId,
  artifactRef,
  artifactType,
}: {
  version: ArtifactVersionSummary;
  artifactId: number;
  artifactRef: string;
  artifactType: string;
}) {
  const [showPreview, setShowPreview] = useState(false);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const canPreview = artifactType === "file_text";
  const canDownload =
    artifactType === "file_text" ||
    artifactType === "file_image" ||
    artifactType === "file_binary" ||
    artifactType === "file_datatable";

  const handleDownload = useCallback(async () => {
    setDownloadError(null);
    try {
      await downloadArtifact(
        artifactId,
        artifactRef,
        version.version,
        version.id,
      );
    } catch {
      setDownloadError("Content unavailable or access denied");
    }
  }, [artifactId, artifactRef, version.version, version.id]);

  return (
    <>
      <tr className="hover:bg-gray-50">
        <td className="px-4 py-3 whitespace-nowrap text-sm font-mono text-gray-900">
          v{version.version}
        </td>
        <td className="px-4 py-3 whitespace-nowrap text-sm text-gray-600">
          {version.execution ? (
            <Link
              to={`/executions/${version.execution}`}
              className="text-blue-600 hover:text-blue-800 font-mono"
            >
              #{version.execution}
            </Link>
          ) : (
            "\u2014"
          )}
        </td>
        <td className="px-4 py-3 whitespace-nowrap text-sm text-gray-600">
          {version.content_type || "\u2014"}
        </td>
        <td className="px-4 py-3 whitespace-nowrap text-sm text-gray-600">
          {formatBytes(version.size_bytes)}
        </td>
        <td className="px-4 py-3 whitespace-nowrap text-sm text-gray-600">
          {version.created_by || "\u2014"}
        </td>
        <td className="px-4 py-3 whitespace-nowrap text-sm text-gray-600">
          {formatDate(version.created)}
        </td>
        <td className="px-4 py-3 whitespace-nowrap text-right">
          <div className="flex items-center justify-end gap-2">
            {canPreview && (
              <button
                onClick={() => setShowPreview(!showPreview)}
                className="text-gray-500 hover:text-blue-600"
                title={showPreview ? "Hide preview" : "Preview content"}
              >
                {showPreview ? (
                  <X className="h-4 w-4" />
                ) : (
                  <FileText className="h-4 w-4" />
                )}
              </button>
            )}
            {canDownload && (
              <button
                onClick={handleDownload}
                className="text-gray-500 hover:text-blue-600"
                title="Download this version"
              >
                <Download className="h-4 w-4" />
              </button>
            )}
          </div>
        </td>
      </tr>
      {downloadError && (
        <tr>
          <td colSpan={7} className="px-4 py-3 text-sm text-red-600">
            {downloadError}
          </td>
        </tr>
      )}
      {showPreview && (
        <tr>
          <td colSpan={7} className="px-4 py-3">
            <TextContentViewer
              artifactId={artifactId}
              versionNumber={version.version}
              versionId={version.id}
              label={`v${version.version}`}
            />
          </td>
        </tr>
      )}
    </>
  );
}

// ============================================================================
// Detail card
// ============================================================================

function MetadataField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <dt className="text-sm font-medium text-gray-500">{label}</dt>
      <dd className="mt-1 text-sm text-gray-900">{children}</dd>
    </div>
  );
}

function ArtifactMetadata({
  artifact,
  versionNumber,
  versionId,
  selectionLabel,
  selectionValid,
}: {
  artifact: ArtifactResponse;
  versionNumber?: number;
  versionId?: number;
  selectionLabel: string;
  selectionValid: boolean;
}) {
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const typeBadge = getArtifactTypeBadge(artifact.type);
  const scopeBadge = getScopeBadge(artifact.scope);

  return (
    <div className="bg-white shadow rounded-lg overflow-hidden">
      <div className="px-6 py-4 border-b border-gray-200">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            {getArtifactTypeIcon(artifact.type)}
            <div>
              <h2 className="text-xl font-bold text-gray-900">
                {artifact.name || artifact.ref}
              </h2>
              {artifact.name && (
                <p className="text-sm text-gray-500 font-mono">
                  {artifact.ref}
                </p>
              )}
            </div>
          </div>
          <div className="flex items-center gap-3">
            {selectionValid && isDownloadable(artifact.type) && (
              <button
                onClick={() => {
                  setDownloadError(null);
                  void downloadArtifact(
                    artifact.id,
                    artifact.ref,
                    versionNumber,
                    versionId,
                  ).catch(() =>
                    setDownloadError("Content unavailable or access denied"),
                  );
                }}
                className="flex items-center gap-2 px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm"
              >
                <Download className="h-4 w-4" />
                Download {selectionLabel}
              </button>
            )}
          </div>
        </div>
      </div>

      <div className="px-6 py-5">
        {downloadError && (
          <p className="mb-3 text-sm text-red-600">{downloadError}</p>
        )}
        <dl className="grid grid-cols-2 md:grid-cols-4 gap-x-6 gap-y-4">
          <MetadataField label="Type">
            <span
              className={`px-2 py-0.5 inline-flex text-xs leading-5 font-semibold rounded-full ${typeBadge.classes}`}
            >
              {typeBadge.label}
            </span>
          </MetadataField>

          <MetadataField label="Visibility">
            <div className="flex items-center gap-1.5">
              {artifact.visibility === "public" ? (
                <>
                  <Eye className="h-4 w-4 text-green-600" />
                  <span className="text-green-700">Public</span>
                </>
              ) : (
                <>
                  <EyeOff className="h-4 w-4 text-gray-400" />
                  <span className="text-gray-600">Private</span>
                </>
              )}
            </div>
          </MetadataField>

          <MetadataField label="Scope">
            <span
              className={`px-2 py-0.5 inline-flex text-xs leading-5 font-semibold rounded-full ${scopeBadge.classes}`}
            >
              {scopeBadge.label}
            </span>
          </MetadataField>

          <MetadataField label="Owner">
            <span className="font-mono text-sm">
              {artifact.owner || "\u2014"}
            </span>
          </MetadataField>

          <MetadataField label="Content Type">
            <span className="font-mono text-xs">
              {artifact.content_type || "\u2014"}
            </span>
          </MetadataField>

          <MetadataField label="Size">
            {formatBytes(artifact.size_bytes)}
          </MetadataField>

          <MetadataField label="Retention">
            {artifact.retention_limit} {artifact.retention_policy}
          </MetadataField>

          <MetadataField label="Created">
            <div className="flex items-center gap-1.5">
              <Clock className="h-3.5 w-3.5 text-gray-400" />
              {formatDate(artifact.created)}
            </div>
          </MetadataField>

          <MetadataField label="Updated">
            <div className="flex items-center gap-1.5">
              <Clock className="h-3.5 w-3.5 text-gray-400" />
              {formatDate(artifact.updated)}
            </div>
          </MetadataField>

          {artifact.description && (
            <div className="col-span-2">
              <MetadataField label="Description">
                {artifact.description}
              </MetadataField>
            </div>
          )}
        </dl>
      </div>
    </div>
  );
}

// ============================================================================
// Versions list
// ============================================================================

function ArtifactVersionsList({
  artifact,
  query,
  browse,
  livePaused,
  onToggleLivePaused,
  isConnected,
}: {
  artifact: ArtifactResponse;
  query: ReturnType<typeof useArtifactVersions>;
  browse: ReturnType<typeof useArtifactCursorPage>;
  livePaused: boolean;
  onToggleLivePaused: () => void;
  isConnected: boolean;
}) {
  const pageSize = 20;
  const { data, isLoading, error } = query;
  const versions = data?.items ?? [];

  return (
    <div className="bg-white shadow rounded-lg overflow-hidden">
      <div className="px-6 py-4 border-b border-gray-200">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <Hash className="h-5 w-5 text-gray-400" />
            <h3 className="text-lg font-semibold text-gray-900">
              Versions
              {versions.length > 0 && (
                <span className="ml-2 text-sm font-normal text-gray-500">
                  ({versions.length} shown)
                </span>
              )}
            </h3>
          </div>
          <LiveStreamControl
            paused={livePaused}
            onTogglePaused={onToggleLivePaused}
            connected={isConnected}
            maxItems={pageSize}
            itemLabel="versions"
            showRetentionHint={false}
          />
        </div>
      </div>

      {isLoading ? (
        <div className="p-8 text-center">
          <Loader2 className="h-6 w-6 animate-spin mx-auto text-blue-600" />
          <p className="mt-2 text-sm text-gray-600">Loading versions...</p>
        </div>
      ) : error ? (
        <div className="p-8 text-center">
          <p className="text-red-600">Failed to load versions</p>
          <p className="text-sm text-gray-600 mt-1">
            {error instanceof Error ? error.message : "Unknown error"}
          </p>
        </div>
      ) : versions.length === 0 ? (
        <div className="p-8 text-center">
          <p className="text-gray-500">No versions yet</p>
        </div>
      ) : (
        <div className="overflow-x-auto">
          <table className="min-w-full divide-y divide-gray-200">
            <thead className="bg-gray-50">
              <tr>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                  Version
                </th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                  Execution
                </th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                  Content Type
                </th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                  Size
                </th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                  Created By
                </th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                  Created
                </th>
                <th className="px-4 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody className="bg-white divide-y divide-gray-200">
              {versions.map((version) => (
                <VersionRow
                  key={version.id}
                  version={version}
                  artifactId={artifact.id}
                  artifactRef={artifact.ref}
                  artifactType={artifact.type}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
      {data && !error && (
        <ArtifactCursorControls
          hasPrevious={browse.hasPrevious}
          hasMore={data.has_more && !!data.next_cursor}
          onPrevious={browse.previous}
          onNext={() => {
            if (data.next_cursor) browse.next(data.next_cursor);
          }}
          disabled={isLoading}
        />
      )}
      <p className="px-6 py-3 text-xs text-gray-500">
        Live refreshes and resuming updates return to page one.
      </p>
    </div>
  );
}

// ============================================================================
// Inline content preview (progress / text for latest)
// ============================================================================

function InlineContentPreview({
  artifact,
  versionNumber,
  versionId,
  selectionLabel,
}: {
  artifact: ArtifactResponse;
  versionNumber?: number;
  versionId?: number;
  selectionLabel: string;
}) {
  if (artifact.type === "progress") {
    return (
      <div className="bg-white shadow rounded-lg overflow-hidden">
        <div className="px-6 py-4 border-b border-gray-200">
          <h3 className="text-lg font-semibold text-gray-900">
            Progress Details
          </h3>
        </div>
        <div className="px-6 py-5">
          <ProgressViewer data={artifact.data} />
        </div>
      </div>
    );
  }

  if (artifact.type === "file_text") {
    return (
      <div className="bg-white shadow rounded-lg overflow-hidden">
        <div className="px-6 py-4 border-b border-gray-200">
          <h3 className="text-lg font-semibold text-gray-900">
            Content Preview ({selectionLabel})
          </h3>
        </div>
        <div className="px-6 py-5">
          <TextContentViewer
            key={`${artifact.id}:${versionNumber ?? "latest"}:${versionId ?? ""}`}
            artifactId={artifact.id}
            versionNumber={versionNumber}
            versionId={versionId}
            label="content"
          />
        </div>
      </div>
    );
  }

  if (artifact.type === "url" && artifact.data) {
    const urlValue =
      typeof artifact.data === "string"
        ? artifact.data
        : typeof artifact.data === "object" &&
            artifact.data !== null &&
            "url" in (artifact.data as Record<string, unknown>)
          ? String((artifact.data as Record<string, unknown>).url)
          : null;

    if (urlValue) {
      return (
        <div className="bg-white shadow rounded-lg overflow-hidden">
          <div className="px-6 py-4 border-b border-gray-200">
            <h3 className="text-lg font-semibold text-gray-900">URL</h3>
          </div>
          <div className="px-6 py-5">
            <a
              href={urlValue}
              target="_blank"
              rel="noopener noreferrer"
              className="text-blue-600 hover:text-blue-800 underline break-all"
            >
              {urlValue}
            </a>
          </div>
        </div>
      );
    }
  }

  // JSON data preview for other types that have data
  if (artifact.data != null) {
    return (
      <div className="bg-white shadow rounded-lg overflow-hidden">
        <div className="px-6 py-4 border-b border-gray-200">
          <h3 className="text-lg font-semibold text-gray-900">Data</h3>
        </div>
        <div className="px-6 py-5">
          <pre className="max-h-96 overflow-y-auto bg-gray-900 text-gray-100 rounded-lg p-4 text-xs font-mono whitespace-pre-wrap break-all">
            {JSON.stringify(artifact.data, null, 2)}
          </pre>
        </div>
      </div>
    );
  }

  return null;
}

// ============================================================================
// Main page
// ============================================================================

export default function ArtifactDetailPage() {
  const { id } = useParams<{ id: string }>();
  const artifactId = id ? Number(id) : undefined;
  const [searchParams] = useSearchParams();
  const rawVersion = searchParams.get("version");
  const rawVersionId = searchParams.get("version_id");
  const rawExecution = searchParams.get("execution");
  const versionNumber =
    rawVersion && /^\d+$/.test(rawVersion) && Number(rawVersion) > 0
      ? Number(rawVersion)
      : undefined;
  const versionId =
    rawVersionId && /^\d+$/.test(rawVersionId) && Number(rawVersionId) > 0
      ? Number(rawVersionId)
      : undefined;
  const selectionValid =
    ((rawVersion === null && rawVersionId === null) ||
      (versionNumber !== undefined && versionId !== undefined)) &&
    (rawExecution === null ||
      (/^\d+$/.test(rawExecution) &&
        Number(rawExecution) > 0 &&
        versionNumber !== undefined));
  const selectionLabel = versionNumber
    ? `${rawExecution ? `Execution #${rawExecution}, ` : ""}v${versionNumber}`
    : "Latest";

  const [livePaused, setLivePaused] = useState(false);
  const browse = useArtifactCursorPage("versions", livePaused, artifactId);
  // History authorizes independently and only needs the route ID, not a header.
  const versionsQuery = useArtifactVersions(artifactId, livePaused, {
    cursor: browse.cursor,
    perPage: 20,
  });
  const { data, isLoading, error } = useArtifact(artifactId, false, livePaused);
  const artifact = data?.data;
  const selectedVersion = versionsQuery.data?.items.find(
    (version) => version.version === versionNumber && version.id === versionId,
  );

  // Subscribe to real-time updates for this artifact
  const { isConnected } = useArtifactStream({
    artifactId,
    executionId: rawExecution ? Number(rawExecution) : undefined,
    enabled: true,
    paused: livePaused,
  });

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <Loader2 className="h-8 w-8 animate-spin text-blue-600" />
          <p className="ml-3 text-gray-600">Loading artifact...</p>
        </div>
      </div>
    );
  }

  if (error || !artifact) {
    return (
      <div className="p-6">
        <div className="mb-6">
          <Link
            to="/artifacts"
            className="flex items-center gap-2 text-gray-600 hover:text-gray-900"
          >
            <ArrowLeft className="h-4 w-4" />
            Back to Artifacts
          </Link>
        </div>
        <div className="bg-white shadow rounded-lg p-12 text-center">
          <p className="text-red-600 text-lg">
            {error ? "Failed to load artifact" : "Artifact not found"}
          </p>
          {error && (
            <p className="text-sm text-gray-600 mt-2">
              {error instanceof Error ? error.message : "Unknown error"}
            </p>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="p-6">
      {/* Back link */}
      <div className="mb-6">
        <Link
          to="/artifacts"
          className="flex items-center gap-2 text-gray-600 hover:text-gray-900 text-sm"
        >
          <ArrowLeft className="h-4 w-4" />
          Back to Artifacts
        </Link>
      </div>

      {/* Metadata card */}
      <ArtifactMetadata
        artifact={
          artifact.classification === "runtime_log"
            ? {
                ...artifact,
                content_type: selectedVersion?.content_type,
                size_bytes: selectedVersion?.size_bytes,
              }
            : artifact
        }
        versionNumber={versionNumber}
        versionId={versionId}
        selectionLabel={selectionLabel}
        selectionValid={selectionValid}
      />

      {/* Inline content preview */}
      <div className="mt-6">
        {selectionValid ? (
          <InlineContentPreview
            artifact={artifact}
            versionNumber={versionNumber}
            versionId={versionId}
            selectionLabel={selectionLabel}
          />
        ) : (
          <p className="text-sm text-red-600">
            Content unavailable or access denied
          </p>
        )}
      </div>

      {/* Versions list */}
      <div className="mt-6">
        <ArtifactVersionsList
          artifact={artifact}
          query={versionsQuery}
          browse={browse}
          livePaused={livePaused}
          onToggleLivePaused={() => setLivePaused((paused) => !paused)}
          isConnected={isConnected}
        />
      </div>
    </div>
  );
}
