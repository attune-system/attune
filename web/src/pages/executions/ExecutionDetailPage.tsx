import { useParams, Link } from "react-router-dom";

/** Format a duration in ms to a human-readable string. */
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
import {
  useExecution,
  useCancelExecution,
  useRescheduleExecution,
} from "@/hooks/useExecutions";
import { useAction } from "@/hooks/useActions";
import { useIdentity, usePermissionSets } from "@/hooks/usePermissions";
import { useEnforcement, useEvent } from "@/hooks/useEvents";
import { useWorker } from "@/hooks/useWorkers";
import { useExecutionStream } from "@/hooks/useExecutionStream";
import { useExecutionHistory } from "@/hooks/useHistory";
import { formatDistanceToNow } from "date-fns";
import { ExecutionStatus } from "@/api";
import type {
  ActionResponse,
  EnforcementResponse,
  EventResponse,
  ExecutionResponse,
  PermissionSetSummary,
} from "@/api";
import { useState, useMemo, type ReactNode } from "react";
import { ChevronDown, RotateCcw, Loader2, XCircle, RefreshCw } from "lucide-react";
import {
  JsonValueDisplay,
  SchemaValueRows,
} from "@/components/common/CuratedDataPanel";
import {
  hasSchemaFields,
  isJsonObject,
} from "@/components/common/curatedDataUtils";
import ExecuteActionModal from "@/components/common/ExecuteActionModal";
import EntityHistoryPanel from "@/components/common/EntityHistoryPanel";
import ExecutionArtifactsPanel from "@/components/executions/ExecutionArtifactsPanel";
import ExecutionProgressBar from "@/components/executions/ExecutionProgressBar";
import WorkflowDetailsPanel from "@/components/executions/WorkflowDetailsPanel";
import { STANDARD_EXECUTION_ACCESS_REF } from "@/lib/permissions";

const getStatusColor = (status: string) => {
  switch (status) {
    case "succeeded":
    case "completed":
      return "bg-green-100 text-green-800";
    case "failed":
      return "bg-red-100 text-red-800";
    case "running":
      return "bg-blue-100 text-blue-800";
    case "pending":
    case "requested":
    case "scheduling":
    case "scheduled":
      return "bg-yellow-100 text-yellow-800";
    case "timeout":
      return "bg-orange-100 text-orange-800";
    case "canceling":
    case "cancelled":
      return "bg-gray-100 text-gray-800";
    case "abandoned":
      return "bg-red-100 text-red-600";
    default:
      return "bg-gray-100 text-gray-800";
  }
};

const getEnforcementStatusColor = (status: string) => {
  switch (status) {
    case "processed":
      return "bg-green-100 text-green-800";
    case "disabled":
      return "bg-gray-100 text-gray-800";
    case "created":
      return "bg-blue-100 text-blue-800";
    default:
      return "bg-gray-100 text-gray-800";
  }
};

/** Map status to a dot color for the timeline. */
const getTimelineDotColor = (status: string) => {
  switch (status) {
    case "completed":
      return "bg-green-500";
    case "failed":
      return "bg-red-500";
    case "running":
      return "bg-blue-500";
    case "requested":
    case "scheduling":
    case "scheduled":
      return "bg-yellow-500";
    case "timeout":
      return "bg-orange-500";
    case "canceling":
    case "cancelled":
      return "bg-gray-400";
    case "abandoned":
      return "bg-red-400";
    default:
      return "bg-gray-400";
  }
};

/** Human-readable label for a status value. */
const getStatusLabel = (status: string) => {
  switch (status) {
    case "requested":
      return "Requested";
    case "scheduling":
      return "Scheduling";
    case "scheduled":
      return "Scheduled";
    case "running":
      return "Running";
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "canceling":
      return "Canceling";
    case "cancelled":
      return "Cancelled";
    case "timeout":
      return "Timed Out";
    case "abandoned":
      return "Abandoned";
    default:
      return status.charAt(0).toUpperCase() + status.slice(1);
  }
};

interface TimelineEntry {
  status: string;
  time: string;
  isInitial: boolean;
}

export default function ExecutionDetailPage() {
  const { id } = useParams<{ id: string }>();
  const { data: executionData, isLoading, error } = useExecution(Number(id));
  const execution = executionData?.data as ExecutionResponse | undefined;
  const { data: initiatorData } = useIdentity(execution?.executor ?? 0);
  const { data: workerData } = useWorker(execution?.worker);
  const {
    data: enforcementData,
    isLoading: enforcementLoading,
    error: enforcementError,
  } = useEnforcement(execution?.enforcement ?? 0);
  const enforcement = enforcementData?.data as EnforcementResponse | undefined;
  const {
    data: eventData,
    isLoading: eventLoading,
    error: eventError,
  } = useEvent(enforcement?.event ? Number(enforcement.event) : 0);
  const event = eventData?.data as EventResponse | undefined;

  // Fetch the action so we can get param_schema for the re-run modal
  const { data: actionData } = useAction(execution?.action_ref || "");

  // Determine if this execution is a workflow (action has workflow_def)
  const isWorkflow = !!actionData?.data?.workflow_def;
  // Actions that may spawn child executions via the MCP server should also
  // surface a child-execution panel even though they are not formal workflows.
  const hasChildExecutions = isWorkflow || !!actionData?.data?.accesses_mcp;

  const [showRerunModal, setShowRerunModal] = useState(false);
  const cancelExecution = useCancelExecution();
  const rescheduleExecution = useRescheduleExecution();

  // Fetch status history for the timeline
  const { data: historyData, isLoading: historyLoading } = useExecutionHistory(
    Number(id),
    { page_size: 100 },
  );

  // Build timeline entries from history records
  const timelineEntries = useMemo<TimelineEntry[]>(() => {
    const records = historyData?.items ?? [];
    const entries: TimelineEntry[] = [];

    for (const record of records) {
      if (record.operation === "INSERT" && record.new_values?.status) {
        entries.push({
          status: String(record.new_values.status),
          time: record.time,
          isInitial: true,
        });
      } else if (
        record.operation === "UPDATE" &&
        record.changed_fields.includes("status") &&
        record.new_values?.status
      ) {
        entries.push({
          status: String(record.new_values.status),
          time: record.time,
          isInitial: false,
        });
      }
    }

    // History comes newest-first; reverse to chronological order
    entries.reverse();
    return entries;
  }, [historyData]);

  // Subscribe to real-time updates for this execution
  const { isConnected } = useExecutionStream({
    executionId: Number(id),
    enabled: !!id,
  });

  if (isLoading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center h-64">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
        </div>
      </div>
    );
  }

  if (error || !execution) {
    return (
      <div className="p-6">
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          <p>
            Error: {error ? (error as Error).message : "Execution not found"}
          </p>
        </div>
        <Link
          to="/executions"
          className="mt-4 inline-block text-blue-600 hover:text-blue-800"
        >
          ← Back to Executions
        </Link>
      </div>
    );
  }

  const isRunning =
    execution.status === ExecutionStatus.RUNNING ||
    execution.status === ExecutionStatus.SCHEDULING ||
    execution.status === ExecutionStatus.SCHEDULED ||
    execution.status === ExecutionStatus.REQUESTED ||
    execution.status === ExecutionStatus.CANCELING;

  const isCancellable = isRunning;
  const isReschedulable = execution.status === ExecutionStatus.REQUESTED;

  return (
    <div className="p-6 max-w-7xl mx-auto">
      {/* Header */}
      <div className="mb-6">
        <Link
          to="/executions"
          className="text-blue-600 hover:text-blue-800 mb-2 inline-block"
        >
          ← Back to Executions
        </Link>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <h1 className="text-3xl font-bold">Execution #{execution.id}</h1>
            {isWorkflow && (
              <span className="px-3 py-1 text-sm rounded-full bg-indigo-100 text-indigo-800">
                Workflow
              </span>
            )}
            {!isWorkflow && actionData?.data?.accesses_mcp && (
              <span
                className="px-3 py-1 text-sm rounded-full bg-purple-100 text-purple-800"
                title="This action may invoke the Attune MCP server and spawn child executions"
              >
                MCP
              </span>
            )}
            <span
              className={`px-3 py-1 text-sm rounded-full ${getStatusColor(execution.status)}`}
            >
              {execution.status}
            </span>
            {isRunning && (
              <div className="flex items-center gap-2 text-sm text-gray-600">
                <div className="animate-spin rounded-full h-4 w-4 border-b-2 border-blue-600" />
                <span>In Progress</span>
              </div>
            )}
            {isConnected && (
              <div className="flex items-center gap-2 text-xs text-green-600">
                <div className="h-2 w-2 rounded-full bg-green-600 animate-pulse" />
                <span>Live</span>
              </div>
            )}
          </div>
          <div className="flex items-center gap-2">
            {isReschedulable && (
              <button
                onClick={() => {
                  if (
                    window.confirm(
                      `Republish the scheduler request for execution #${execution.id}? This is only for requested executions that appear stuck.`,
                    )
                  ) {
                    rescheduleExecution.mutate(execution.id);
                  }
                }}
                disabled={rescheduleExecution.isPending}
                className="px-4 py-2 bg-amber-600 text-white rounded hover:bg-amber-700 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
                title="Republish this requested execution for scheduler recovery"
              >
                {rescheduleExecution.isPending ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <RefreshCw className="h-4 w-4" />
                )}
                Reschedule
              </button>
            )}
            {isCancellable && (
              <button
                onClick={() => {
                  if (
                    window.confirm(
                      `Are you sure you want to cancel execution #${execution.id}?`,
                    )
                  ) {
                    cancelExecution.mutate(execution.id);
                  }
                }}
                disabled={cancelExecution.isPending}
                className="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
                title="Cancel this execution"
              >
                {cancelExecution.isPending ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <XCircle className="h-4 w-4" />
                )}
                Cancel
              </button>
            )}
            <button
              onClick={() => setShowRerunModal(true)}
              disabled={!actionData?.data}
              className="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
              title={
                !actionData?.data
                  ? "Loading action details..."
                  : "Re-run this action with the same parameters"
              }
            >
              <RotateCcw className="h-4 w-4" />
              Re-Run
            </button>
          </div>
        </div>
        <p className="text-gray-600 mt-2">
          <Link
            to={`/actions/${execution.action_ref}`}
            className="text-blue-600 hover:text-blue-800"
          >
            {execution.action_ref}
          </Link>
        </p>
        {execution.workflow_task && (
          <p className="text-sm text-indigo-600 mt-1 flex items-center gap-1.5">
            <span className="text-gray-500">Task</span>{" "}
            <span className="font-medium">
              {execution.workflow_task.task_name}
            </span>
            {execution.parent && (
              <>
                <span className="text-gray-500">in workflow</span>
                <Link
                  to={`/executions/${execution.parent}`}
                  className="text-indigo-600 hover:text-indigo-800 font-medium"
                >
                  Execution #{execution.parent}
                </Link>
              </>
            )}
          </p>
        )}
      </div>

      {/* Child execution details — combined timeline + tasks panel.
          Shown for workflows and for actions flagged as accessing MCP. */}
      {hasChildExecutions && (
        <div className="mb-6">
          <WorkflowDetailsPanel
            parentExecution={execution}
            actionRef={execution.action_ref}
            title={isWorkflow ? "Workflow Details" : "Agent Session Details"}
            tasksTabLabel={isWorkflow ? "Tasks" : "Attune MCP Tool Calls"}
          />
        </div>
      )}

      {/* Re-Run Modal */}
      {showRerunModal && actionData?.data && (
        <ExecuteActionModal
          action={actionData.data}
          onClose={() => setShowRerunModal(false)}
          initialParameters={execution.config}
          initialPermissionSetRefs={execution.permission_set_refs}
          initialTimeoutSeconds={execution.timeout_seconds}
        />
      )}

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Main Content */}
        <div className="lg:col-span-2 space-y-6">
          {/* Status & Timing */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-xl font-semibold mb-4">Execution Details</h2>
            <dl className="grid grid-cols-1 sm:grid-cols-2 gap-4">
              <div>
                <dt className="text-sm font-medium text-gray-500">Status</dt>
                <dd className="mt-1">
                  <span
                    className={`px-2 py-1 text-xs rounded ${getStatusColor(execution.status)}`}
                  >
                    {execution.status}
                  </span>
                </dd>
              </div>

              <div>
                <dt className="text-sm font-medium text-gray-500">Created</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(execution.created).toLocaleString()}
                  <span className="text-gray-500 ml-2 text-xs">
                    (
                    {formatDistanceToNow(new Date(execution.created), {
                      addSuffix: true,
                    })}
                    )
                  </span>
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-gray-500">Updated</dt>
                <dd className="mt-1 text-sm text-gray-900">
                  {new Date(execution.updated).toLocaleString()}
                </dd>
              </div>
              {execution.enforcement && (
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Enforcement ID
                  </dt>
                  <dd className="mt-1 text-sm text-gray-900">
                    {execution.enforcement}
                  </dd>
                </div>
              )}
              {execution.parent && (
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Parent Execution
                  </dt>
                  <dd className="mt-1 text-sm text-gray-900">
                    <Link
                      to={`/executions/${execution.parent}`}
                      className="text-blue-600 hover:text-blue-800"
                    >
                      #{execution.parent}
                    </Link>
                  </dd>
                </div>
              )}
              {execution.executor && (
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Initiated By
                  </dt>
                  <dd className="mt-1 text-sm text-gray-900">
                    {initiatorData?.data ? (
                      <div className="space-y-0.5">
                        <Link
                          to={`/access-control/identities/${execution.executor}`}
                          className="text-blue-600 hover:text-blue-800 font-medium"
                        >
                          {initiatorData.data.display_name ||
                            initiatorData.data.login}
                        </Link>
                        <div className="text-xs text-gray-500">
                          {initiatorData.data.display_name && (
                            <span>@{initiatorData.data.login} · </span>
                          )}
                          ID {execution.executor}
                        </div>
                      </div>
                    ) : (
                      <>Identity #{execution.executor}</>
                    )}
                  </dd>
                </div>
              )}
              {execution.worker && (
                <div>
                  <dt className="text-sm font-medium text-gray-500">
                    Worker ID
                  </dt>
                  <dd className="mt-1 text-sm text-gray-900">
                    {workerData ? (
                      <div className="space-y-0.5">
                        <div className="font-medium">{workerData.name}</div>
                        <div className="text-xs text-gray-500">
                          ID {execution.worker} · {workerData.worker_role} ·{" "}
                          {workerData.health_state}
                        </div>
                      </div>
                    ) : (
                      <>Worker #{execution.worker}</>
                    )}
                  </dd>
                </div>
              )}
            </dl>

            {/* Configuration — merged inline */}
            {isJsonObject(execution.config) &&
              Object.keys(execution.config).length > 0 && (
                <div className="mt-6 border-t border-gray-200 pt-4">
                  <h3 className="text-sm font-semibold text-gray-700 mb-2">
                    Configuration
                  </h3>
                  <p className="text-xs text-gray-500 mb-3">
                    Parameters used for this execution, annotated with the
                    action parameter schema when available.
                  </p>
                  <SchemaValueRows
                    schema={
                      (actionData?.data as ActionResponse | undefined)
                        ?.param_schema
                    }
                    values={execution.config}
                    emptyMessage="No configuration parameters were captured for this execution."
                    maskSecrets
                    hideUnprovided
                  />
                </div>
              )}

            {/* Inline progress bar (visible when execution has progress artifacts) */}
            {isRunning && (
              <ExecutionProgressBar
                executionId={execution.id}
                isRunning={isRunning}
              />
            )}
          </div>

          {execution.permission_set_refs &&
            execution.permission_set_refs.length > 0 && (
              <ExecutionTokenAccessCard
                permissionSetRefs={execution.permission_set_refs}
              />
            )}

          <ExecutionResultCard
            result={execution.result}
            action={actionData?.data as ActionResponse | undefined}
            isWorkflow={isWorkflow}
          />
        </div>

        {/* Sidebar */}
        <div className="space-y-6">
          {/* Timeline */}
          <div className="bg-white shadow rounded-lg p-6">
            <h2 className="text-lg font-semibold mb-4">Timeline</h2>

            {historyLoading && (
              <div className="flex items-center justify-center py-6">
                <Loader2 className="h-5 w-5 animate-spin text-gray-400" />
                <span className="ml-2 text-sm text-gray-500">
                  Loading timeline…
                </span>
              </div>
            )}

            {!historyLoading && timelineEntries.length === 0 && (
              /* Fallback: no history data yet — show basic created/current status */
              <div className="space-y-4">
                <div className="flex gap-4">
                  <div className="flex flex-col items-center">
                    <div
                      className={`w-3 h-3 rounded-full ${getTimelineDotColor(execution.status)}`}
                    />
                  </div>
                  <div className="flex-1">
                    <p className="font-medium text-sm">
                      {getStatusLabel(execution.status)}
                    </p>
                    <p className="text-xs text-gray-500">
                      {new Date(execution.created).toLocaleString()}
                    </p>
                  </div>
                </div>
              </div>
            )}

            {!historyLoading && timelineEntries.length > 0 && (
              <div className="space-y-0">
                {timelineEntries.map((entry, idx) => {
                  const isLast = idx === timelineEntries.length - 1;
                  const time = new Date(entry.time);
                  const prevTime =
                    idx > 0 ? new Date(timelineEntries[idx - 1].time) : null;
                  const durationMs = prevTime
                    ? time.getTime() - prevTime.getTime()
                    : null;

                  return (
                    <div key={`${entry.status}-${idx}`} className="flex gap-3">
                      <div className="flex flex-col items-center">
                        <div
                          className={`w-2.5 h-2.5 rounded-full flex-shrink-0 ${getTimelineDotColor(entry.status)}${
                            isLast && isRunning ? " animate-pulse" : ""
                          }`}
                        />
                        {!isLast && (
                          <div className="w-0.5 flex-1 min-h-[20px] bg-gray-200" />
                        )}
                      </div>
                      <div className={`flex-1 ${!isLast ? "pb-3" : ""}`}>
                        <div className="flex items-center gap-2">
                          <p className="font-medium text-sm">
                            {getStatusLabel(entry.status)}
                          </p>
                          <span
                            className={`px-1.5 py-0.5 text-[10px] font-medium rounded ${getStatusColor(entry.status)}`}
                          >
                            {entry.status}
                          </span>
                        </div>
                        <p className="text-xs text-gray-500">
                          {time.toLocaleString()}
                          <span className="text-gray-400 ml-1 text-[10px]">
                            (
                            {formatDistanceToNow(time, { addSuffix: true })}
                            )
                          </span>
                        </p>
                        {durationMs !== null && durationMs > 0 && (
                          <p className="text-[10px] text-gray-400 mt-0.5">
                            +{formatDuration(durationMs)} since previous
                          </p>
                        )}
                      </div>
                    </div>
                  );
                })}

                {isRunning && (
                  <div className="flex gap-3 pt-3">
                    <div className="flex flex-col items-center">
                      <div className="w-2.5 h-2.5 rounded-full bg-blue-500 animate-pulse" />
                    </div>
                    <div className="flex-1">
                      <p className="font-medium text-sm text-blue-600">
                        In Progress…
                      </p>
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>

          <ExecutionTriggerContextPanel
            enforcementId={execution.enforcement ?? null}
            enforcement={enforcement}
            event={event}
            isLoading={enforcementLoading}
            eventLoading={eventLoading}
            error={enforcementError}
            eventError={eventError}
          />

          {/* Artifacts */}
          <ExecutionArtifactsPanel
            executionId={execution.id}
            isRunning={isRunning}
          />
        </div>
      </div>

      {/* Change History */}
      <div className="mt-6">
        <EntityHistoryPanel
          entityType="execution"
          entityId={execution.id}
          title="Execution History"
        />
      </div>
    </div>
  );
}

function ExecutionTriggerContextPanel({
  enforcementId,
  enforcement,
  event,
  isLoading,
  eventLoading,
  error,
  eventError,
}: {
  enforcementId: number | null;
  enforcement?: EnforcementResponse;
  event?: EventResponse;
  isLoading: boolean;
  eventLoading: boolean;
  error: unknown;
  eventError: unknown;
}) {
  if (!enforcementId) return null;

  return (
    <div className="bg-white shadow rounded-lg p-6">
      <h2 className="text-lg font-semibold mb-1">Trigger Context</h2>
      <p className="text-xs text-gray-500 mb-4">
        Rule enforcement and event data that caused this execution to be
        requested.
      </p>

      {isLoading && (
        <div className="flex items-center py-4 text-sm text-gray-500">
          <Loader2 className="h-4 w-4 animate-spin mr-2" />
          Loading enforcement…
        </div>
      )}

      {!isLoading && (Boolean(error) || !enforcement) && (
        <div className="rounded border border-yellow-200 bg-yellow-50 p-3 text-sm text-yellow-800">
          Enforcement #{enforcementId} could not be loaded.
        </div>
      )}

      {enforcement && (
        <div className="space-y-4 text-sm">
          <div className="rounded-lg border border-gray-200 p-3">
            <div className="flex flex-wrap items-center gap-2">
              <Link
                to={`/enforcements/${enforcement.id}`}
                className="font-medium text-blue-600 hover:text-blue-800"
              >
                Enforcement #{enforcement.id}
              </Link>
              <span
                className={`px-2 py-0.5 text-xs rounded ${getEnforcementStatusColor(enforcement.status)}`}
              >
                {enforcement.status}
              </span>
              <span className="px-2 py-0.5 text-xs rounded bg-purple-50 text-purple-700">
                {enforcement.condition}
              </span>
            </div>
            <dl className="mt-3 space-y-2">
              <CompactDetail label="Rule">
                {enforcement.rule_ref ? (
                  <Link
                    to={`/rules/${enforcement.rule_ref}`}
                    className="text-blue-600 hover:text-blue-800"
                  >
                    {enforcement.rule_ref}
                  </Link>
                ) : (
                  <span className="text-gray-500">N/A</span>
                )}
              </CompactDetail>
              <CompactDetail label="Trigger">
                <Link
                  to={`/triggers/${enforcement.trigger_ref}`}
                  className="text-blue-600 hover:text-blue-800"
                >
                  {enforcement.trigger_ref}
                </Link>
              </CompactDetail>
              <CompactDetail label="Resolved">
                {enforcement.resolved_at
                  ? new Date(enforcement.resolved_at).toLocaleString()
                  : "Pending"}
              </CompactDetail>
            </dl>
          </div>

          {Object.keys(enforcement.conditions ?? {}).length > 0 && (
            <CompactJsonSection
              title="Rule Conditions"
              value={enforcement.conditions}
            />
          )}

          {isJsonObject(enforcement.config) &&
            Object.keys(enforcement.config).length > 0 && (
              <CompactJsonSection
                title="Resolved Action Parameters"
                value={enforcement.config}
              />
            )}

          {Object.keys(enforcement.payload ?? {}).length > 0 && (
            <CompactJsonSection
              title="Enforcement Payload Snapshot"
              value={enforcement.payload}
            />
          )}

          {eventLoading && (
            <div className="flex items-center py-2 text-sm text-gray-500">
              <Loader2 className="h-4 w-4 animate-spin mr-2" />
              Loading event…
            </div>
          )}

          {!eventLoading && Boolean(eventError) && (
            <div className="rounded border border-yellow-200 bg-yellow-50 p-3 text-sm text-yellow-800">
              Event #{enforcement.event} could not be loaded.
            </div>
          )}

          {!eventLoading &&
            !eventError &&
            !event &&
            !enforcement.event && (
              <div className="rounded border border-gray-200 bg-gray-50 p-3 text-sm text-gray-600">
                This enforcement has no linked event row.
              </div>
            )}

          {event && (
            <div className="rounded-lg border border-gray-200 p-3">
              <div className="flex flex-wrap items-center gap-2">
                <Link
                  to={`/events/${event.id}`}
                  className="font-medium text-blue-600 hover:text-blue-800"
                >
                  Event #{event.id}
                </Link>
                <span className="px-2 py-0.5 text-xs rounded bg-blue-50 text-blue-700">
                  {event.trigger_ref}
                </span>
              </div>
              <dl className="mt-3 space-y-2">
                <CompactDetail label="Created">
                  {new Date(event.created).toLocaleString()}
                </CompactDetail>
                <CompactDetail label="Source">
                  {event.source_ref || "N/A"}
                </CompactDetail>
              </dl>
              {Object.keys(event.payload ?? {}).length > 0 && (
                <div className="mt-3">
                  <CompactJsonSection
                    title="Event Payload"
                    value={event.payload}
                  />
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function CompactDetail({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div>
      <dt className="text-xs font-medium text-gray-500">{label}</dt>
      <dd className="mt-0.5 text-gray-900 break-words">{children}</dd>
    </div>
  );
}

function CompactJsonSection({
  title,
  value,
}: {
  title: string;
  value: unknown;
}) {
  return (
    <div>
      <h3 className="text-xs font-semibold uppercase tracking-wide text-gray-500">
        {title}
      </h3>
      <div className="mt-1 max-h-56 overflow-auto">
        <JsonValueDisplay value={value} />
      </div>
    </div>
  );
}


function ExecutionResultCard({
  result,
  action,
  isWorkflow,
}: {
  result: unknown;
  action?: ActionResponse;
  isWorkflow: boolean;
}) {
  if (!isJsonObject(result) || Object.keys(result).length === 0) {
    return null;
  }

  const exitCode =
    typeof result.exit_code === "number" ? result.exit_code : undefined;
  const succeeded =
    typeof result.succeeded === "boolean"
      ? result.succeeded
      : exitCode !== undefined
        ? exitCode === 0
        : undefined;
  const durationMs =
    typeof result.duration_ms === "number" ? result.duration_ms : undefined;
  const hasProcessEnvelope =
    !isWorkflow &&
    ("succeeded" in result ||
      "exit_code" in result ||
      "stdout" in result ||
      "stderr_log" in result ||
      "duration_ms" in result);
  const data = "data" in result ? result.data : undefined;
  const remainingEntries = Object.entries(result).filter(
    ([key]) =>
      ![
        "succeeded",
        "exit_code",
        "duration_ms",
        "stdout",
        "stderr_log",
        "error",
        "data",
        "stdout_truncated",
        "stdout_bytes_truncated",
        "stderr_truncated",
        "stderr_bytes_truncated",
      ].includes(key),
  );

  return (
    <div className="bg-white shadow rounded-lg p-6">
      <h2 className="text-xl font-semibold mb-4">Result</h2>

      {hasProcessEnvelope && (
        <div className="mb-4 rounded-lg border border-gray-200 p-3">
          <div className="flex flex-wrap items-center gap-3">
            {succeeded !== undefined && (
              <span
                className={`rounded px-2 py-1 text-sm font-medium ${
                  succeeded
                    ? "bg-green-100 text-green-800"
                    : "bg-red-100 text-red-800"
                }`}
              >
                {succeeded ? "Succeeded" : "Failed"}
                {exitCode !== undefined ? ` (exit ${exitCode})` : ""}
              </span>
            )}
            {durationMs !== undefined && (
              <span className="text-sm text-gray-600">
                Duration: {formatDuration(durationMs)}
              </span>
            )}
          </div>
          {typeof result.error === "string" && result.error.trim() && (
            <p className="mt-3 rounded bg-red-50 px-3 py-2 text-sm text-red-700">
              {result.error}
            </p>
          )}
        </div>
      )}

      {"data" in result && (
        <div className="mb-4">
          <h3 className="text-sm font-semibold text-gray-700 mb-2">
            Output Data
          </h3>
          {isJsonObject(data) && hasSchemaFields(action?.out_schema) ? (
            <SchemaValueRows
              schema={action?.out_schema}
              values={data}
              emptyMessage="No structured output data was returned."
            />
          ) : (
            <JsonValueDisplay value={data} />
          )}
        </div>
      )}

      {typeof result.stdout === "string" && result.stdout.length > 0 && (
        <div className="mb-4">
          <div className="flex items-center gap-2 mb-2">
            <h3 className="text-sm font-semibold text-gray-700">Stdout</h3>
            {result.stdout_truncated === true && (
              <span className="rounded bg-yellow-50 px-2 py-0.5 text-xs text-yellow-700">
                truncated
              </span>
            )}
          </div>
          <pre className="max-h-72 overflow-auto whitespace-pre-wrap rounded bg-gray-900 p-3 text-xs text-gray-100">
            {result.stdout}
          </pre>
        </div>
      )}

      {typeof result.stderr_log === "string" && (
        <div className="mb-4 rounded bg-gray-50 px-3 py-2 text-sm text-gray-700">
          <span className="font-medium">Stderr log:</span>{" "}
          <span className="font-mono">{result.stderr_log}</span>
          {result.stderr_truncated === true && (
            <span className="ml-2 rounded bg-yellow-50 px-2 py-0.5 text-xs text-yellow-700">
              truncated
            </span>
          )}
        </div>
      )}

      {remainingEntries.length > 0 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-700 mb-2">
            Additional Result Fields
          </h3>
          <SchemaValueRows
            schema={null}
            values={Object.fromEntries(remainingEntries)}
            emptyMessage="No additional result fields."
          />
        </div>
      )}

      {!hasProcessEnvelope &&
        !("data" in result) &&
        remainingEntries.length === 0 && <JsonValueDisplay value={result} />}
    </div>
  );
}

function summarizeGrant(grant: unknown): string {
  if (!grant || typeof grant !== "object") {
    return "Invalid grant";
  }

  const grantObject = grant as {
    resource?: unknown;
    actions?: unknown;
    constraints?: unknown;
  };
  const resource =
    typeof grantObject.resource === "string" ? grantObject.resource : "unknown";
  const actions = Array.isArray(grantObject.actions)
    ? grantObject.actions.filter((action): action is string => typeof action === "string")
    : [];
  const actionText = actions.length > 0 ? actions.join(", ") : "no actions";

  if (
    !grantObject.constraints ||
    typeof grantObject.constraints !== "object" ||
    Array.isArray(grantObject.constraints)
  ) {
    return `${resource}: ${actionText}`;
  }

  const constraints = grantObject.constraints as Record<string, unknown>;
  const constraintParts: string[] = [];
  for (const [key, value] of Object.entries(constraints)) {
    if (key === "ids") {
      continue;
    }
    if (Array.isArray(value)) {
      constraintParts.push(`${key}=${value.join(", ")}`);
    } else if (value !== null && value !== undefined) {
      constraintParts.push(`${key}=${String(value)}`);
    }
  }

  return constraintParts.length > 0
    ? `${resource}: ${actionText} (${constraintParts.join("; ")})`
    : `${resource}: ${actionText}`;
}

function ExecutionTokenAccessCard({
  permissionSetRefs,
}: {
  permissionSetRefs: string[];
}) {
  const [isGrantDetailsOpen, setIsGrantDetailsOpen] = useState(false);
  const {
    data: permissionSets,
    isLoading,
    error: permissionSetsError,
  } = usePermissionSets(null, { enabled: isGrantDetailsOpen });
  const permissionSetsByRef = useMemo(
    () =>
      new Map(
        (permissionSets ?? []).map((permissionSet: PermissionSetSummary) => [
          permissionSet.ref,
          permissionSet,
        ]),
      ),
    [permissionSets],
  );

  return (
    <div className="bg-white shadow rounded-lg p-6">
      <h2 className="text-xl font-semibold mb-2">Effective Token Access</h2>
      <p className="text-sm text-gray-600 mb-4">
        This execution has an Attune API token scoped to the permission set refs
        below.
      </p>

      <div className="flex flex-wrap gap-2 mb-4">
        {permissionSetRefs.map((ref) =>
          ref === STANDARD_EXECUTION_ACCESS_REF ? (
            <span
              key={ref}
              className="font-mono text-xs font-medium text-green-700 bg-green-50 rounded px-2 py-1"
              title="Standard action/pack-scoped keys and artifacts access"
            >
              {ref}
            </span>
          ) : (
            <Link
              key={ref}
              to={`/access-control/permission-sets/${ref}`}
              className="font-mono text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded px-2 py-1"
            >
              {ref}
            </Link>
          ),
        )}
      </div>

      <div className="border border-gray-200 rounded-lg">
        <button
          type="button"
          onClick={() => setIsGrantDetailsOpen((open) => !open)}
          className="flex w-full items-center justify-between gap-3 px-3 py-2 text-left hover:bg-gray-50"
          aria-expanded={isGrantDetailsOpen}
        >
          <span className="text-sm font-medium text-gray-700">
            Grant details
          </span>
          <ChevronDown
            className={`h-4 w-4 flex-shrink-0 text-gray-400 transition-transform ${
              isGrantDetailsOpen ? "rotate-180" : ""
            }`}
          />
        </button>

        {isGrantDetailsOpen && (
          <div className="space-y-3 border-t border-gray-200 p-3">
            {permissionSetRefs.map((ref) => {
              if (ref === STANDARD_EXECUTION_ACCESS_REF) {
                return (
                  <div key={ref} className="border border-green-200 rounded-lg p-3 bg-green-50">
                    <div className="font-mono text-sm font-medium text-green-700">
                      {ref}
                    </div>
                    <p className="text-xs text-green-700 mt-1">
                      Grants this execution access to keys and artifacts scoped
                      to the executing action/pack, plus the workflow action/pack
                      when this is a workflow task execution.
                    </p>
                  </div>
                );
              }

              const permissionSet = permissionSetsByRef.get(ref);
              const grants = Array.isArray(permissionSet?.grants)
                ? permissionSet.grants
                : [];

              return (
                <div key={ref} className="border border-gray-200 rounded-lg p-3">
                  <div className="flex items-center justify-between gap-3">
                    <Link
                      to={`/access-control/permission-sets/${ref}`}
                      className="font-mono text-sm font-medium text-blue-600 hover:text-blue-800"
                    >
                      {ref}
                    </Link>
                    {!permissionSet && !isLoading && !permissionSetsError && (
                      <span className="text-xs px-2 py-0.5 rounded bg-yellow-100 text-yellow-800">
                        Not found
                      </span>
                    )}
                  </div>

                  {isLoading ? (
                    <p className="text-xs text-gray-500 mt-2">
                      Loading grants...
                    </p>
                  ) : permissionSetsError ? (
                    <p className="text-xs text-gray-500 mt-2">
                      Grant details are unavailable with your current
                      permissions.
                    </p>
                  ) : grants.length > 0 ? (
                    <ul className="mt-2 space-y-1">
                      {grants.map((grant, index) => (
                        <li
                          key={`${ref}-${index}`}
                          className="text-xs font-mono text-gray-700 bg-gray-50 rounded px-2 py-1"
                        >
                          {summarizeGrant(grant)}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="text-xs text-gray-500 mt-2">
                      No grants available for this permission set ref.
                    </p>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
