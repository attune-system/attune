import { useState, useMemo } from "react";
import { Link } from "react-router-dom";
import { formatDistanceToNow } from "date-fns";
import {
  ChevronDown,
  ChevronRight,
  Workflow,
  ChartGantt,
  List,
  CheckCircle2,
  XCircle,
  Clock,
  Loader2,
  AlertTriangle,
  Ban,
  CircleDot,
  RotateCcw,
} from "lucide-react";
import type { ExecutionSummary } from "@/api";
import {
  useChildExecutions,
  useWorkflowCacheIterations,
  type WorkflowCacheIteration,
} from "@/hooks/useExecutions";
import { useExecutionStream } from "@/hooks/useExecutionStream";
import WorkflowTimelineDAG, {
  type ParentExecutionInfo,
} from "@/components/executions/workflow-timeline";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type TabId = "timeline" | "tasks";

interface WorkflowDetailsPanelProps {
  /** The parent (workflow) execution */
  parentExecution: ParentExecutionInfo;
  /** The action_ref of the parent execution (used to fetch workflow def) */
  actionRef: string;
  /** Whether the panel starts collapsed (default: false) */
  defaultCollapsed?: boolean;
  /** Which tab to show initially (default: "timeline") */
  defaultTab?: TabId;
  /** Heading shown on the panel (default: "Workflow Details") */
  title?: string;
  /** Label for the tabular child-list tab (default: "Tasks") */
  tasksTabLabel?: string;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

function getStatusIcon(status: string) {
  switch (status) {
    case "completed":
      return <CheckCircle2 className="h-4 w-4 text-green-500" />;
    case "failed":
      return <XCircle className="h-4 w-4 text-red-500" />;
    case "running":
      return <Loader2 className="h-4 w-4 text-blue-500 animate-spin" />;
    case "requested":
    case "scheduling":
    case "scheduled":
      return <Clock className="h-4 w-4 text-yellow-500" />;
    case "timeout":
      return <AlertTriangle className="h-4 w-4 text-orange-500" />;
    case "canceling":
    case "cancelled":
      return <Ban className="h-4 w-4 text-gray-400" />;
    case "abandoned":
      return <AlertTriangle className="h-4 w-4 text-red-400" />;
    default:
      return <CircleDot className="h-4 w-4 text-gray-400" />;
  }
}

function getStatusBadgeClasses(status: string): string {
  switch (status) {
    case "completed":
      return "bg-green-100 text-green-800";
    case "failed":
      return "bg-red-100 text-red-800";
    case "running":
    case "scanning":
      return "bg-blue-100 text-blue-800";
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
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/**
 * Combined "Workflow Details" panel that sits at the top of the execution
 * detail page for workflow executions. Contains two tabs:
 *   - **Timeline** — the Gantt-style WorkflowTimelineDAG
 *   - **Tasks** — the tabular list of child task executions
 */
export default function WorkflowDetailsPanel({
  parentExecution,
  actionRef,
  defaultCollapsed = false,
  defaultTab = "timeline",
  title = "Workflow Details",
  tasksTabLabel = "Tasks",
}: WorkflowDetailsPanelProps) {
  const [isCollapsed, setIsCollapsed] = useState(defaultCollapsed);
  const [activeTab, setActiveTab] = useState<TabId>(defaultTab);

  // Fetch child executions (shared between both tabs' summary badges)
  const { data, isLoading, error } = useChildExecutions(parentExecution.id, {
    includeDescendants: true,
  });
  const {
    data: cacheIterationData,
    isLoading: cacheIterationsLoading,
    error: cacheIterationsError,
  } = useWorkflowCacheIterations(parentExecution.id);

  // Subscribe to unfiltered execution stream so child execution WebSocket
  // notifications update the query cache in real-time.
  useExecutionStream({ enabled: true });

  const tasks = useMemo(() => data?.items ?? [], [data]);
  const cacheIterations = cacheIterationData?.data ?? [];
  const cacheIterationsUnsupported = cacheIterationData?.unsupported ?? false;

  // Order tasks so descendants appear nested under their parent. We sort by:
  //   1. immediate children of the panel's root execution first (in id order)
  //   2. for each non-root task, place it directly after its parent in the list
  // This produces a stable, depth-first ordering that mirrors the call tree.
  const orderedTasks = useMemo(() => {
    if (tasks.length === 0) return tasks;
    const byId = new Map(tasks.map((t) => [t.id, t] as const));
    const childrenByParent = new Map<number, typeof tasks>();
    for (const t of tasks) {
      const p = t.parent ?? -1;
      const arr = childrenByParent.get(p) ?? [];
      arr.push(t);
      childrenByParent.set(p, arr);
    }
    for (const arr of childrenByParent.values()) {
      arr.sort((a, b) => a.id - b.id);
    }
    const out: typeof tasks = [];
    const visit = (id: number) => {
      const kids = childrenByParent.get(id) ?? [];
      for (const k of kids) {
        out.push(k);
        visit(k.id);
      }
    };
    visit(parentExecution.id);
    // Anything still unreached (e.g. orphaned by a deleted intermediate) gets
    // appended at the end so the user still sees it.
    if (out.length < tasks.length) {
      const seen = new Set(out.map((t) => t.id));
      for (const t of tasks) if (!seen.has(t.id)) out.push(t);
    }
    void byId;
    return out;
  }, [tasks, parentExecution.id]);

  const summary = useMemo(() => {
    const total = tasks.length;
    const completed = tasks.filter((t) => t.status === "completed").length;
    const failed = tasks.filter((t) => t.status === "failed").length;
    const running = tasks.filter(
      (t) =>
        t.status === "running" ||
        t.status === "requested" ||
        t.status === "scheduling" ||
        t.status === "scheduled",
    ).length;
    const other = total - completed - failed - running;
    return { total, completed, failed, running, other };
  }, [tasks]);

  // Don't render at all if there are no children and we're done loading
  if (
    !isLoading &&
    !cacheIterationsLoading &&
    tasks.length === 0 &&
    cacheIterations.length === 0 &&
    !error &&
    !cacheIterationsError
  ) {
    return null;
  }

  return (
    <div className="bg-white shadow rounded-lg">
      {/* ----------------------------------------------------------------- */}
      {/* Header row: collapse toggle + title + summary badges              */}
      {/* ----------------------------------------------------------------- */}
      <button
        onClick={() => setIsCollapsed(!isCollapsed)}
        className="w-full flex items-center justify-between px-6 py-4 text-left hover:bg-gray-50 rounded-t-lg transition-colors"
      >
        <div className="flex items-center gap-3">
          {isCollapsed ? (
            <ChevronRight className="h-5 w-5 text-gray-400" />
          ) : (
            <ChevronDown className="h-5 w-5 text-gray-400" />
          )}
          <Workflow className="h-5 w-5 text-indigo-500" />
          <h2 className="text-xl font-semibold">{title}</h2>
          {!isLoading && (
            <span className="text-sm text-gray-500">
              ({summary.total} task{summary.total !== 1 ? "s" : ""})
            </span>
          )}
        </div>

        {/* Summary badges (always visible) */}
        <div className="flex items-center gap-2">
          {summary.completed > 0 && (
            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800">
              <CheckCircle2 className="h-3 w-3" />
              {summary.completed}
            </span>
          )}
          {summary.running > 0 && (
            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800">
              <Loader2 className="h-3 w-3 animate-spin" />
              {summary.running}
            </span>
          )}
          {summary.failed > 0 && (
            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-red-100 text-red-800">
              <XCircle className="h-3 w-3" />
              {summary.failed}
            </span>
          )}
          {summary.other > 0 && (
            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-700">
              {summary.other}
            </span>
          )}
        </div>
      </button>

      {/* ----------------------------------------------------------------- */}
      {/* Body (collapsible)                                                */}
      {/* ----------------------------------------------------------------- */}
      {!isCollapsed && (
        <div className="border-t border-gray-100">
          {!cacheIterationsUnsupported &&
            (cacheIterationsLoading ||
              cacheIterationsError ||
              cacheIterations.length > 0) && (
              <CacheIterationsStatus
                iterations={cacheIterations}
                isLoading={cacheIterationsLoading}
                error={cacheIterationsError}
              />
            )}

          {/* Tab bar */}
          <div className="flex items-center gap-1 px-6 pt-3 pb-0">
            <TabButton
              active={activeTab === "timeline"}
              onClick={() => setActiveTab("timeline")}
              icon={<ChartGantt className="h-4 w-4" />}
              label="Timeline"
            />
            <TabButton
              active={activeTab === "tasks"}
              onClick={() => setActiveTab("tasks")}
              icon={<List className="h-4 w-4" />}
              label={tasksTabLabel}
            />
          </div>

          {/* Tab content — both tabs stay mounted so the timeline's
              ResizeObserver remains active and containerWidth never resets. */}
          <div className={activeTab === "timeline" ? "" : "hidden"}>
            <WorkflowTimelineDAG
              parentExecution={parentExecution}
              actionRef={actionRef}
              embedded
            />
          </div>
          <div className={activeTab === "tasks" ? "" : "hidden"}>
            <TasksTab
              tasks={orderedTasks}
              rootId={parentExecution.id}
              isLoading={isLoading}
              error={error}
            />
          </div>
        </div>
      )}
    </div>
  );
}

const MAX_ERROR_SUMMARY_LENGTH = 512;

function boundedErrorSummary(summary: string): string {
  if (summary.length <= MAX_ERROR_SUMMARY_LENGTH) return summary;
  return `${summary.slice(0, MAX_ERROR_SUMMARY_LENGTH - 1)}…`;
}

function formatTimestamp(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString();
}

function CacheIterationsStatus({
  iterations,
  isLoading,
  error,
}: {
  iterations: WorkflowCacheIteration[];
  isLoading: boolean;
  error: unknown;
}) {
  return (
    <section
      aria-label="Workflow cache iterations"
      className="border-b border-gray-100 px-6 py-3"
    >
      <div className="mb-2 flex items-center gap-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-gray-500">
          Cache iterations
        </h3>
        {isLoading && (
          <Loader2 className="h-3.5 w-3.5 animate-spin text-gray-400" />
        )}
      </div>

      {!!error && (
        <p className="text-xs text-gray-500">
          Cache iteration status unavailable.
        </p>
      )}

      {!error && isLoading && iterations.length === 0 && (
        <p className="text-xs text-gray-500">Loading cache iteration status…</p>
      )}

      {iterations.length > 0 && (
        <div className="space-y-2">
          {iterations.map((iteration) => (
            <div
              key={`${iteration.task_name}-${iteration.generation_id}`}
              className="rounded-md border border-gray-200 bg-gray-50/70 px-3 py-2"
            >
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
                <span className="font-medium text-gray-900">
                  {iteration.task_name}
                </span>
                <span
                  className={`rounded-full px-2 py-0.5 font-medium ${getStatusBadgeClasses(iteration.state)}`}
                >
                  {iteration.state}
                </span>
                <span className="text-gray-500">
                  Generation #{iteration.generation_id}
                </span>
                <span className="text-gray-600">
                  Scanned {iteration.scanned_count.toLocaleString()}
                </span>
                <span className="text-gray-600">
                  Dispatched {iteration.dispatched_count.toLocaleString()}
                </span>
                <span className="text-gray-500">
                  Batch {iteration.batch_size.toLocaleString()} · Page{" "}
                  {iteration.page_size.toLocaleString()} · Concurrency{" "}
                  {iteration.concurrency.toLocaleString()}
                </span>
              </div>
              <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-gray-500">
                <span>
                  Created{" "}
                  <time dateTime={iteration.created}>
                    {formatTimestamp(iteration.created)}
                  </time>
                </span>
                <span>
                  Updated{" "}
                  <time dateTime={iteration.updated}>
                    {formatTimestamp(iteration.updated)}
                  </time>
                </span>
                {iteration.completed_at && (
                  <span>
                    Completed{" "}
                    <time dateTime={iteration.completed_at}>
                      {formatTimestamp(iteration.completed_at)}
                    </time>
                  </span>
                )}
              </div>
              {iteration.error_summary && (
                <p className="mt-1 break-words text-xs text-red-700">
                  {boundedErrorSummary(iteration.error_summary)}
                </p>
              )}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Tab Button
// ---------------------------------------------------------------------------

function TabButton({
  active,
  onClick,
  icon,
  label,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
}) {
  return (
    <button
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      className={`
        flex items-center gap-1.5 px-3 py-2 text-sm font-medium rounded-t-md
        transition-colors border-b-2
        ${
          active
            ? "text-indigo-700 border-indigo-500 bg-indigo-50/50"
            : "text-gray-500 border-transparent hover:text-gray-700 hover:bg-gray-50"
        }
      `}
    >
      {icon}
      {label}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Tasks Tab — table of child task executions
// ---------------------------------------------------------------------------

function TasksTab({
  tasks,
  rootId,
  isLoading,
  error,
}: {
  tasks: ExecutionSummary[];
  rootId: number;
  isLoading: boolean;
  error: unknown;
}) {
  // Build a depth map so we can indent rows that are nested below the root.
  const depthById = new Map<number, number>([[rootId, 0]]);
  for (const t of tasks) {
    const parentDepth = t.parent != null ? depthById.get(t.parent) : undefined;
    depthById.set(t.id, parentDepth != null ? parentDepth + 1 : 1);
  }
  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-8">
        <Loader2 className="h-5 w-5 animate-spin text-gray-400" />
        <span className="ml-2 text-sm text-gray-500">
          Loading workflow tasks…
        </span>
      </div>
    );
  }

  if (error) {
    return (
      <div className="mx-6 my-4 bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded text-sm">
        Error loading workflow tasks:{" "}
        {error instanceof Error ? error.message : "Unknown error"}
      </div>
    );
  }

  if (tasks.length === 0) {
    return (
      <div className="flex items-center justify-center py-8 text-sm text-gray-500">
        No workflow tasks yet.
      </div>
    );
  }

  return (
    <div className="px-6 pb-6 pt-2">
      <div className="space-y-2">
        {/* Column headers */}
        <div className="grid grid-cols-12 gap-3 px-3 py-2 text-xs font-medium text-gray-500 uppercase tracking-wider border-b border-gray-100">
          <div className="col-span-1">#</div>
          <div className="col-span-3">Task</div>
          <div className="col-span-3">Action</div>
          <div className="col-span-2">Status</div>
          <div className="col-span-2">Duration</div>
          <div className="col-span-1">Retry</div>
        </div>

        {/* Task rows */}
        {tasks.map((task, idx) => {
          const wt = task.workflow_task;
          const depth = Math.max(0, (depthById.get(task.id) ?? 1) - 1);
          const isMcpChild = !wt && depth > 0;
          const taskName =
            wt?.task_name ??
            task.action_ref.split(".").pop() ??
            `Task ${idx + 1}`;
          const retryCount = wt?.retry_count ?? 0;
          const maxRetries = wt?.max_retries ?? 0;
          const timedOut = wt?.timed_out ?? false;

          // Compute duration from started_at → updated (actual run time)
          const startedAt = task.started_at ? new Date(task.started_at) : null;
          const created = new Date(task.created);
          const updated = new Date(task.updated);
          const isTerminal =
            task.status === "completed" ||
            task.status === "failed" ||
            task.status === "timeout";
          const durationMs =
            wt?.duration_ms ??
            (isTerminal && startedAt
              ? updated.getTime() - startedAt.getTime()
              : null);

          return (
            <Link
              key={task.id}
              to={`/executions/${task.id}`}
              className="grid grid-cols-12 gap-3 px-3 py-3 rounded-lg hover:bg-gray-50 transition-colors items-center group"
            >
              {/* Index */}
              <div className="col-span-1 text-sm text-gray-400 font-mono">
                {idx + 1}
              </div>

              {/* Task name */}
              <div
                className="col-span-3 flex items-center gap-2 min-w-0"
                style={{ paddingLeft: depth * 18 }}
              >
                {getStatusIcon(task.status)}
                <span
                  className="text-sm font-medium text-gray-900 truncate group-hover:text-blue-600"
                  title={taskName}
                >
                  {taskName}
                </span>
                {wt?.task_index != null && (
                  <span className="text-xs text-gray-400 flex-shrink-0">
                    [{wt.task_index}]
                  </span>
                )}
                {isMcpChild && (
                  <span
                    className="text-[10px] flex-shrink-0 px-1.5 py-0.5 rounded bg-purple-100 text-purple-800 uppercase tracking-wide"
                    title="Spawned via MCP from a parent execution"
                  >
                    MCP
                  </span>
                )}
              </div>

              {/* Action ref */}
              <div className="col-span-3 min-w-0">
                <span
                  className="text-sm text-gray-600 truncate block"
                  title={task.action_ref}
                >
                  {task.action_ref}
                </span>
              </div>

              {/* Status badge */}
              <div className="col-span-2 flex items-center gap-1.5">
                <span
                  className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium ${getStatusBadgeClasses(task.status)}`}
                >
                  {task.status}
                </span>
                {timedOut && (
                  <span title="Timed out">
                    <AlertTriangle className="h-3.5 w-3.5 text-orange-500" />
                  </span>
                )}
              </div>

              {/* Duration */}
              <div className="col-span-2 text-sm text-gray-500">
                {task.status === "running" ? (
                  <span className="text-blue-600">
                    {formatDistanceToNow(startedAt ?? created, {
                      addSuffix: false,
                    })}
                    …
                  </span>
                ) : durationMs != null && durationMs > 0 ? (
                  formatDuration(durationMs)
                ) : (
                  <span className="text-gray-300">—</span>
                )}
              </div>

              {/* Retry info */}
              <div className="col-span-1 text-sm text-gray-500">
                {maxRetries > 0 ? (
                  <span
                    className="inline-flex items-center gap-0.5"
                    title={`Attempt ${retryCount + 1} of ${maxRetries + 1}`}
                  >
                    <RotateCcw className="h-3 w-3" />
                    {retryCount}/{maxRetries}
                  </span>
                ) : (
                  <span className="text-gray-300">—</span>
                )}
              </div>
            </Link>
          );
        })}
      </div>
    </div>
  );
}
