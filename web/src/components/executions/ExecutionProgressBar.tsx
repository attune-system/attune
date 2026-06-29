import { useMemo } from "react";
import { BarChart3 } from "lucide-react";
import {
  useExecutionArtifacts,
  type ArtifactSummary,
} from "@/hooks/useArtifacts";
import {
  useArtifactStream,
  useArtifactProgress,
} from "@/hooks/useArtifactStream";

interface ExecutionProgressBarProps {
  executionId: number;
  /** Whether the execution is still running (enables real-time updates) */
  isRunning: boolean;
}

/**
 * Inline progress bar for executions that have progress-type artifacts.
 *
 * Combines two data sources for responsiveness:
 * 1. **Polling**: `useExecutionArtifacts` fetches the artifact list periodically
 *    so we can detect when a progress artifact first appears and read its initial state.
 * 2. **WebSocket**: `useArtifactStream` subscribes to real-time `artifact_updated`
 *    notifications, which include the latest `progress_percent` and `progress_message`
 *    extracted by the database trigger — providing instant updates between polls.
 *
 * The WebSocket-pushed summary takes precedence when available (it's newer), with
 * the polled data as a fallback for the initial render before any WS message arrives.
 *
 * Renders nothing if no progress artifact exists for this execution.
 */
export default function ExecutionProgressBar({
  executionId,
  isRunning,
}: ExecutionProgressBarProps) {
  // Subscribe to real-time artifact updates for this execution
  useArtifactStream({ executionId, enabled: isRunning });

  // Read the latest progress pushed via WebSocket (no API call)
  const wsSummary = useArtifactProgress(executionId);

  // Poll-based artifact list (fallback + initial detection)
  const { data } = useExecutionArtifacts(executionId, isRunning);

  // Find progress artifacts from the polled data
  const progressArtifact = useMemo<ArtifactSummary | null>(() => {
    const artifacts = data?.data ?? [];
    return artifacts.find((a) => a.type === "progress") ?? null;
  }, [data]);

  // If there's no progress artifact at all, render nothing
  if (!progressArtifact && !wsSummary) {
    return null;
  }

  // Prefer the WS-pushed summary (more current), fall back to indicating
  // that a progress artifact exists but we haven't received detail yet.
  const percent = wsSummary?.percent ?? null;
  const message = wsSummary?.message ?? null;
  const name = wsSummary?.name ?? progressArtifact?.name ?? "Progress";

  // If we have a progress artifact but no percent yet (first poll, no WS yet),
  // show an indeterminate state
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
