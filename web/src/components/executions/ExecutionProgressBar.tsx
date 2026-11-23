import { useMemo } from "react";
import { BarChart3 } from "lucide-react";
import {
  useArtifact,
  useExecutionArtifacts,
  type ArtifactSummary,
} from "@/hooks/useArtifacts";
import { useArtifactStream } from "@/hooks/useArtifactStream";

interface ExecutionProgressBarProps {
  executionId: number;
  /** Whether the execution is still running (enables real-time updates) */
  isRunning: boolean;
}

/**
 * Inline progress bar for executions that have progress-type artifacts.
 *
 * Artifact notifications refresh the execution-scoped artifact list and
 * detail cache. The authenticated API supplies the displayed progress.
 *
 * Renders nothing if no progress artifact exists for this execution.
 */
export default function ExecutionProgressBar({
  executionId,
  isRunning,
}: ExecutionProgressBarProps) {
  // Subscribe to real-time artifact updates for this execution
  useArtifactStream({ executionId, enabled: isRunning });

  const { data } = useExecutionArtifacts(executionId, isRunning, {
    type: "progress",
    perPage: 1,
  });

  // Find progress artifacts from the polled data
  const progressArtifact = useMemo<ArtifactSummary | null>(() => {
    const artifacts = data?.items ?? [];
    return artifacts.find((a) => a.type === "progress") ?? null;
  }, [data]);
  const { data: progressDetail } = useArtifact(progressArtifact?.id, isRunning);
  const detail = progressDetail?.data;
  const entry = Array.isArray(detail?.data) ? detail.data.at(-1) : undefined;
  const latestEntry =
    entry && typeof entry === "object" && !Array.isArray(entry) ? entry : null;

  // If there's no progress artifact at all, render nothing
  if (!progressArtifact) {
    return null;
  }

  const detailPercent = latestEntry?.percent;
  const percent = typeof detailPercent === "number" ? detailPercent : null;
  const message =
    typeof latestEntry?.message === "string" ? latestEntry.message : null;
  const name = detail?.name ?? progressArtifact?.name ?? "Progress";

  // Show an indeterminate state while loading the artifact detail.
  const hasPercent = percent != null;
  const clampedPercent = hasPercent ? Math.min(Math.max(percent, 0), 100) : 0;
  const isComplete = hasPercent && clampedPercent >= 100;

  return (
    <div className="mt-4 pt-4 border-t border-gray-100">
      <div className="flex items-center gap-2 mb-1.5">
        <BarChart3 className="h-4 w-4 text-amber-500 flex-shrink-0" />
        <span className="text-sm font-medium text-gray-700 truncate">
          {name}
        </span>
        {hasPercent && (
          <span className="text-xs font-mono text-gray-500 ml-auto flex-shrink-0">
            {Math.round(clampedPercent)}%
          </span>
        )}
      </div>

      {/* Progress bar */}
      <div className="w-full bg-gray-200 rounded-full h-2">
        {hasPercent ? (
          <div
            className={`h-2 rounded-full transition-all duration-500 ease-out ${
              isComplete ? "bg-green-500" : "bg-amber-500"
            }`}
            style={{ width: `${clampedPercent}%` }}
          />
        ) : (
          /* Indeterminate shimmer when we know a progress artifact exists
             but haven't received a percent value yet */
          <div className="h-2 rounded-full bg-amber-300 animate-pulse w-full opacity-40" />
        )}
      </div>

      {/* Message */}
      {message && (
        <p className="text-xs text-gray-500 mt-1 truncate" title={message}>
          {message}
        </p>
      )}
    </div>
  );
}
