import { memo, useEffect } from "react";
import { Link } from "react-router-dom";
import { X, ExternalLink, Loader2, XCircle } from "lucide-react";
import { useExecution, useCancelExecution } from "@/hooks/useExecutions";
import { useExecutionStream } from "@/hooks/useExecutionStream";
import { formatDistanceToNow } from "date-fns";
import type { ExecutionStatus } from "@/api";

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const secs = ms / 1000;
  if (secs < 60) return `${secs.toFixed(1)}s`;
  const mins = Math.floor(secs / 60);
  const remainSecs = Math.round(secs % 60);
  if (mins < 60) return `${mins}m ${remainSecs}s`;
  const hrs = Math.floor(mins / 60);
  const remainMins = mins % 60;
  return `${hrs}h ${remainMins}m`;
}

const getStatusColor = (status: string) => {
  switch (status) {
    case "succeeded":
    case "completed":
      return "bg-green-100 text-green-800";
    case "failed":
    case "timeout":
      return "bg-red-100 text-red-800";
    case "running":
      return "bg-blue-100 text-blue-800";
    case "scheduled":
    case "scheduling":
    case "requested":
      return "bg-yellow-100 text-yellow-800";
    case "canceling":
    case "cancelled":
      return "bg-gray-100 text-gray-600";
    default:
      return "bg-gray-100 text-gray-800";
  }
};

interface ExecutionPreviewPanelProps {
  executionId: number;
  onClose: () => void;
}

const ExecutionPreviewPanel = memo(function ExecutionPreviewPanel({
  executionId,
  onClose,
}: ExecutionPreviewPanelProps) {
  const { data, isLoading, error } = useExecution(executionId);
  const execution = data?.data;
  const cancelExecution = useCancelExecution();

  // Subscribe to real-time updates for this execution
  useExecutionStream({ executionId, enabled: true });

  // Close on Escape key
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const isRunning =
    execution?.status === "running" ||
    execution?.status === "scheduling" ||
    execution?.status === "scheduled" ||
    execution?.status === "requested" ||
    execution?.status === "canceling";

  const isCancellable = isRunning;

  const startedAt = execution?.started_at
    ? new Date(execution.started_at)
    : null;
  const created = execution ? new Date(execution.created) : null;
  const updated = execution ? new Date(execution.updated) : null;
  const durationMs =
    startedAt && updated && !isRunning
      ? updated.getTime() - startedAt.getTime()
      : null;

  return (
    <div className="border-l border-gray-200 bg-white flex flex-col h-full overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-gray-200 bg-gray-50 flex-shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <h3 className="text-sm font-semibold text-gray-900 truncate">
            Execution #{executionId}
          </h3>
          {execution && (
            <span
              className={`px-2 py-0.5 text-xs rounded-full font-medium flex-shrink-0 ${getStatusColor(execution.status)}`}
            >
              {execution.status}
            </span>
          )}
          {isRunning && (
            <Loader2 className="h-3.5 w-3.5 text-blue-500 animate-spin flex-shrink-0" />
          )}
        </div>
        <div className="flex items-center gap-1 flex-shrink-0">
          {isCancellable && (
            <button
              onClick={() => {
                if (
                  window.confirm(
                    `Are you sure you want to cancel execution #${executionId}?`,
                  )
                ) {
                  cancelExecution.mutate(executionId);
                }
              }}
              disabled={cancelExecution.isPending}
              className="p-1.5 text-gray-400 hover:text-red-600 rounded hover:bg-red-50 transition-colors"
              title="Cancel execution"
            >
              {cancelExecution.isPending ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <XCircle className="h-4 w-4" />
              )}
            </button>
          )}
          <Link
            to={`/executions/${executionId}`}
            className="p-1.5 text-gray-400 hover:text-blue-600 rounded hover:bg-gray-100 transition-colors"
            title="Open full detail page"
          >
            <ExternalLink className="h-4 w-4" />
          </Link>
          <button
            onClick={onClose}
            className="p-1.5 text-gray-400 hover:text-gray-600 rounded hover:bg-gray-100 transition-colors"
            title="Close preview (Esc)"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto">
        {isLoading && (
          <div className="flex items-center justify-center h-32">
            <Loader2 className="h-6 w-6 animate-spin text-gray-400" />
          </div>
        )}

        {error && !execution && (
          <div className="p-4">
            <div className="bg-red-50 border border-red-200 text-red-700 px-3 py-2 rounded text-sm">
              Error: {(error as Error).message}
            </div>
          </div>
        )}

        {execution && (
          <div className="divide-y divide-gray-100">
            {(() => {
              const executionWithTrace = execution as typeof execution & {
                trace_tag?: string | null;
              };
              return executionWithTrace.trace_tag ? (
                <div className="px-4 py-3">
                  <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Trace Tag
                  </dt>
                  <dd className="mt-0.5 text-sm text-gray-900 font-mono">
                    {executionWithTrace.trace_tag}
                  </dd>
                </div>
              ) : null;
            })()}

            {/* Action */}
            <div className="px-4 py-3">
              <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                Action
              </dt>
              <dd className="mt-1">
                <Link
                  to={`/actions/${execution.action_ref}`}
                  className="text-sm text-blue-600 hover:text-blue-800 font-medium"
                >
                  {execution.action_ref}
                </Link>
              </dd>
            </div>

            {/* Timing */}
            <div className="px-4 py-3 space-y-2">
              <div>
                <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                  Created
                </dt>
                <dd className="mt-0.5 text-sm text-gray-900">
                  {created!.toLocaleString()}
                  <span className="text-gray-400 ml-1.5 text-xs">
                    {formatDistanceToNow(created!, { addSuffix: true })}
                  </span>
                </dd>
              </div>
              {durationMs != null && durationMs > 0 && (
                <div>
                  <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Duration
                  </dt>
                  <dd className="mt-0.5 text-sm text-gray-900">
                    {formatDuration(durationMs)}
                  </dd>
                </div>
              )}
              {isRunning && (
                <div>
                  <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Elapsed
                  </dt>
                  <dd className="mt-0.5 text-blue-600 flex items-center gap-1.5">
                    <Loader2 className="h-3 w-3 animate-spin" />
                    {formatDistanceToNow(startedAt ?? created!)}
                  </dd>
                </div>
              )}
            </div>

            {/* References */}
            <div className="px-4 py-3 space-y-2">
              {execution.parent && (
                <div>
                  <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Parent Execution
                  </dt>
                  <dd className="mt-0.5 text-sm">
                    <Link
                      to={`/executions/${execution.parent}`}
                      className="text-blue-600 hover:text-blue-800 font-mono"
                    >
                      #{execution.parent}
                    </Link>
                  </dd>
                </div>
              )}
              {execution.enforcement && (
                <div>
                  <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Enforcement
                  </dt>
                  <dd className="mt-0.5 text-sm text-gray-900 font-mono">
                    #{execution.enforcement}
                  </dd>
                </div>
              )}
              {execution.executor && (
                <div>
                  <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Initiated By
                  </dt>
                  <dd className="mt-0.5 text-sm text-gray-900 font-mono">
                    #{execution.executor}
                  </dd>
                </div>
              )}
              {execution.worker && (
                <div>
                  <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Worker
                  </dt>
                  <dd className="mt-0.5 text-sm text-gray-900 font-mono">
                    #{execution.worker}
                  </dd>
                </div>
              )}
              {execution.workflow_task && (
                <div>
                  <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Workflow Task
                  </dt>
                  <dd className="mt-0.5 text-sm text-gray-900">
                    <span className="font-medium">
                      {execution.workflow_task.task_name}
                    </span>
                    {execution.workflow_task.task_index != null && (
                      <span className="text-gray-400 ml-1">
                        [{execution.workflow_task.task_index}]
                      </span>
                    )}
                  </dd>
                </div>
              )}
            </div>

            {/* Config / Parameters */}
            {execution.config && Object.keys(execution.config).length > 0 && (
              <div className="px-4 py-3">
                <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide mb-1.5">
                  Parameters
                </dt>
                <dd>
                  <pre className="bg-gray-50 border border-gray-200 rounded p-3 text-xs overflow-x-auto max-h-48 overflow-y-auto">
                    {JSON.stringify(execution.config, null, 2)}
                  </pre>
                </dd>
              </div>
            )}

            {/* Result */}
            {execution.result && Object.keys(execution.result).length > 0 && (
              <div className="px-4 py-3">
                <dt className="text-xs font-medium text-gray-500 uppercase tracking-wide mb-1.5">
                  Result
                </dt>
                <dd>
                  <pre
                    className={`border rounded p-3 text-xs overflow-x-auto max-h-64 overflow-y-auto ${
                      execution.status === ("failed" as ExecutionStatus) ||
                      execution.status === ("timeout" as ExecutionStatus)
                        ? "bg-red-50 border-red-200"
                        : "bg-gray-50 border-gray-200"
                    }`}
                  >
                    {JSON.stringify(execution.result, null, 2)}
                  </pre>
                </dd>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Footer */}
      {execution && (
        <div className="px-4 py-3 border-t border-gray-200 bg-gray-50 flex-shrink-0">
          <Link
            to={`/executions/${executionId}`}
            className="block w-full text-center px-3 py-2 text-sm font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded-md transition-colors"
          >
            Open Full Details
          </Link>
        </div>
      )}
    </div>
  );
});

export default ExecutionPreviewPanel;
