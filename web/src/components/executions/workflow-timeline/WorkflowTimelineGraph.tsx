/**
 * WorkflowTimelineDAG — Orchestrator component for the Prefect-style
 * workflow run timeline visualization.
 *
 * This component:
 *   1. Fetches the workflow definition (for transition metadata)
 *   2. Transforms child execution summaries into timeline structures
 *   3. Computes the DAG layout (lanes, positions, edges)
 *   4. Delegates rendering to TimelineRenderer
 *
 * It is designed to be embedded in the ExecutionDetailPage for workflow
 * executions, receiving child execution data from the parent.
 */

import { useMemo, useRef, useCallback, useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import type { ExecutionSummary } from "@/api";
import { useWorkflow } from "@/hooks/useWorkflows";
import { useChildExecutions } from "@/hooks/useExecutions";
import { useExecutionStream } from "@/hooks/useExecutionStream";
import { useWorkflowTaskWaits } from "@/hooks/useWorkflowTaskWaits";
import {
  ChartGantt,
  ChevronDown,
  ChevronRight,
  Loader2,
  Maximize2,
} from "lucide-react";

import type {
  TimelineTask,
  TimelineEdge,
  TimelineMilestone,
  WorkflowDefinition,
  LayoutConfig,
} from "./types";
import { DEFAULT_LAYOUT, timelineTaskDestinationPath } from "./types";
import {
  buildTimelineTasks,
  buildSyntheticWaitTasks,
  collapseWithItemsGroups,
  buildEdges,
  buildMilestones,
} from "./data";
import { computeLayout } from "./layout";
import TimelineRenderer from "./TimelineRenderer";
import TimelineModal from "./TimelineModal";

// ---------------------------------------------------------------------------
// Minimal parent execution shape accepted by this component.
// Both ExecutionResponse and ExecutionSummary satisfy this interface,
// so callers don't need an ugly cast.
// ---------------------------------------------------------------------------

export interface ParentExecutionInfo {
  id: number;
  action_ref: string;
  status: string;
  created: string;
  updated: string;
  started_at?: string | null;
}

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface WorkflowTimelineDAGProps {
  /** The parent (workflow) execution — accepts ExecutionResponse or ExecutionSummary */
  parentExecution: ParentExecutionInfo;
  /** The action_ref of the parent execution (used to fetch workflow def) */
  actionRef: string;
  /** Whether the panel starts collapsed */
  defaultCollapsed?: boolean;
  /**
   * When true, renders only the timeline content (legend, renderer, modal)
   * without the outer card wrapper, header button, or collapse toggle.
   * Used when the component is embedded inside another panel (e.g. WorkflowDetailsPanel).
   */
  embedded?: boolean;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function WorkflowTimelineGraph({
  parentExecution,
  actionRef,
  defaultCollapsed = false,
  embedded = false,
}: WorkflowTimelineDAGProps) {
  const navigate = useNavigate();
  const containerRef = useRef<HTMLDivElement>(null);
  const [isCollapsed, setIsCollapsed] = useState(
    embedded ? false : defaultCollapsed,
  );
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [containerWidth, setContainerWidth] = useState(900);
  const [nowMs, setNowMs] = useState(Date.now);

  // ---- Determine if the workflow is still in-flight ----
  const isTerminal = [
    "completed",
    "failed",
    "timeout",
    "cancelled",
    "abandoned",
  ].includes(parentExecution.status);

  // ---- Data fetching ----

  // Fetch child executions, including descendants spawned via MCP so that
  // calls into Attune from inside an action surface as nested timeline nodes.
  const { data: childData, isLoading: childrenLoading } = useChildExecutions(
    parentExecution.id,
    { includeDescendants: true },
  );
  const { data: waitsData, isLoading: waitsLoading } = useWorkflowTaskWaits(
    parentExecution.id,
    parentExecution.status,
  );

  // Subscribe to real-time execution updates so child tasks update live
  useExecutionStream({ enabled: true });

  // Fetch workflow definition for transition metadata
  // The workflow ref matches the action ref for workflow actions
  const { data: workflowData } = useWorkflow(actionRef);

  const childExecutions: ExecutionSummary[] = useMemo(() => {
    return childData?.items ?? [];
  }, [childData]);
  const waits = useMemo(() => waitsData?.data ?? [], [waitsData]);
  const hasWaitingTask = waits.some((wait) => wait.state === "waiting");

  // Keep active execution and inquiry-wait bars moving with wall-clock time.
  useEffect(() => {
    if ((isTerminal && !hasWaitingTask) || (!embedded && isCollapsed)) return;
    let rafId: number;
    const tick = () => {
      setNowMs(Date.now());
      rafId = requestAnimationFrame(tick);
    };
    rafId = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafId);
  }, [isTerminal, hasWaitingTask, isCollapsed, embedded]);

  const workflowDef: WorkflowDefinition | null = useMemo(() => {
    if (!workflowData?.data?.definition) return null;
    return workflowData.data.definition as WorkflowDefinition;
  }, [workflowData]);

  // ---- Observe container width for responsive layout ----
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const w = entry.contentRect.width;
        if (w > 0) setContainerWidth(w);
      }
    });

    observer.observe(el);
    return () => observer.disconnect();
  }, [isCollapsed]);

  // ---- Build timeline data structures ----
  // Split into two phases:
  //  1. Structural memo — edges and upstream/downstream links. These depend
  //     only on the set of child executions and the workflow definition, NOT
  //     on the current time. Recomputes only when real data changes.
  //  2. Per-frame memo — task time positions, milestones, and layout. These
  //     depend on `nowMs` so they update every animation frame (~60fps) while
  //     the workflow is running, giving smooth bar growth.

  // Phase 1: Build tasks (without time-dependent endMs) and compute edges.
  // `buildEdges` mutates tasks' upstreamIds/downstreamIds, so we must call
  // it in the same memo that creates the task objects.
  const { structuralTasks, taskEdges } = useMemo(() => {
    // Build individual tasks, then collapse large with_items groups into
    // single synthetic nodes before computing edges.
    const rawTasks = buildTimelineTasks(childExecutions, workflowDef);
    const { tasks: executionTasks, memberToGroup } = collapseWithItemsGroups(
      rawTasks,
      childExecutions,
      workflowDef,
    );
    const syntheticTasks = buildSyntheticWaitTasks({
      waits,
      childExecutions,
      workflowDef,
      parentExecutionId: parentExecution.id,
    });
    const structuralTasks = [...executionTasks, ...syntheticTasks];

    // Derive dependency edges (purely structural — no time dependency).
    // Pass the collapse mapping so edges redirect to group nodes.
    const taskEdges = buildEdges(
      structuralTasks,
      childExecutions,
      workflowDef,
      memberToGroup,
    );

    return { structuralTasks, taskEdges };
  }, [childExecutions, waits, workflowDef, parentExecution.id]);

  // Phase 2: Patch running-task time positions and build milestones.
  // This runs every animation frame while the workflow is active.
  const { tasks, milestones, milestoneEdges, suppressedEdgeKeys } =
    useMemo(() => {
      if (structuralTasks.length === 0) {
        return {
          tasks: [] as TimelineTask[],
          milestones: [] as TimelineMilestone[],
          milestoneEdges: [] as TimelineEdge[],
          suppressedEdgeKeys: new Set<string>(),
        };
      }

      // Patch endMs / durationMs for running tasks so bars grow in real time.
      // We shallow-clone each task that needs updating to keep React diffing
      // efficient (unchanged tasks keep the same object identity).
      const tasks = structuralTasks.map((t) => {
        if (
          (t.state === "running" || t.state === "waiting") &&
          t.startMs != null
        ) {
          const endMs = nowMs;
          return { ...t, endMs, durationMs: endMs - t.startMs };
        }
        return t;
      });

      // Build milestones (start/end diamonds, merge/fork junctions)
      const parentAsSummary: ExecutionSummary = {
        id: parentExecution.id,
        action_ref: parentExecution.action_ref,
        status: parentExecution.status as ExecutionSummary["status"],
        created: parentExecution.created,
        updated: parentExecution.updated,
        started_at: parentExecution.started_at,
      };
      const { milestones, milestoneEdges, suppressedEdgeKeys } =
        buildMilestones(tasks, parentAsSummary);

      return { tasks, milestones, milestoneEdges, suppressedEdgeKeys };
    }, [structuralTasks, parentExecution, nowMs]);

  // ---- Compute layout ----

  const layoutConfig: LayoutConfig = useMemo(() => {
    // Adjust layout based on task count for readability
    const taskCount = tasks.length;
    if (taskCount > 50) {
      return {
        ...DEFAULT_LAYOUT,
        laneHeight: 26,
        barHeight: 16,
        lanePadding: 5,
      };
    }
    if (taskCount > 20) {
      return {
        ...DEFAULT_LAYOUT,
        laneHeight: 30,
        barHeight: 18,
        lanePadding: 6,
      };
    }
    return DEFAULT_LAYOUT;
  }, [tasks.length]);

  const layout = useMemo(() => {
    if (tasks.length === 0) return null;

    return computeLayout(
      tasks,
      taskEdges,
      milestones,
      milestoneEdges,
      containerWidth,
      layoutConfig,
      suppressedEdgeKeys,
    );
  }, [
    tasks,
    taskEdges,
    milestones,
    milestoneEdges,
    containerWidth,
    layoutConfig,
    suppressedEdgeKeys,
  ]);

  // ---- Handlers ----

  const handleTaskClick = useCallback(
    (task: TimelineTask) => {
      const path = timelineTaskDestinationPath(task.destination);
      if (path) navigate(path);
    },
    [navigate],
  );

  // ---- Summary stats ----

  const summary = useMemo(() => {
    const total = tasks.length;
    const completed = tasks.filter((task) => task.state === "completed").length;
    const failed = tasks.filter((task) => task.state === "failed").length;
    const running = tasks.filter(
      (task) => task.state === "running" || task.state === "waiting",
    ).length;
    const other = total - completed - failed - running;

    // Compute overall duration from the already-patched tasks array so we
    // get the live running-task endMs values for free.
    let durationMs: number | null = null;
    const taskStartTimes = tasks
      .filter((t) => t.startMs != null)
      .map((t) => t.startMs!);
    const taskEndTimes = tasks
      .filter((t) => t.endMs != null)
      .map((t) => t.endMs!);

    if (taskStartTimes.length > 0 && taskEndTimes.length > 0) {
      durationMs = Math.max(...taskEndTimes) - Math.min(...taskStartTimes);
    }

    return { total, completed, failed, running, other, durationMs };
  }, [tasks]);

  // ---- Early returns ----

  if ((childrenLoading || waitsLoading) && tasks.length === 0) {
    return (
      <div className={embedded ? "" : "bg-white shadow rounded-lg"}>
        <div className="flex items-center gap-3 p-4">
          <Loader2 className="h-4 w-4 animate-spin text-gray-400" />
          <span className="text-sm text-gray-500">
            Loading workflow timeline…
          </span>
        </div>
      </div>
    );
  }

  if (tasks.length === 0) {
    if (embedded) {
      return (
        <div className="flex items-center justify-center py-8 text-sm text-gray-500">
          No workflow tasks yet.
        </div>
      );
    }
    return null; // No child tasks to display
  }

  // ---- Shared content (legend + renderer + modal) ----
  const timelineContent = (
    <>
      {/* Expand to modal */}
      <div className="flex justify-end px-3 py-1">
        <button
          onClick={(e) => {
            e.stopPropagation();
            setIsModalOpen(true);
          }}
          className="flex items-center gap-1 text-[10px] text-gray-400 hover:text-gray-600 transition-colors"
          title="Open expanded timeline with zoom"
        >
          <Maximize2 className="h-3 w-3" />
          Expand
        </button>
      </div>

      {/* Legend */}
      <div className="flex items-center gap-3 px-5 pb-2 text-[10px] text-gray-400">
        <LegendItem color="#22c55e" label="Completed" />
        <LegendItem color="#3b82f6" label="Running" />
        <LegendItem color="#ef4444" label="Failed" dashed />
        <LegendItem color="#f97316" label="Timeout" dotted />
        <LegendItem color="#9ca3af" label="Pending" />
        <span className="ml-2 text-gray-300">|</span>
        <EdgeLegendItem color="#22c55e" label="Succeeded" />
        <EdgeLegendItem color="#ef4444" label="Failed" dashed />
        <EdgeLegendItem color="#9ca3af" label="Always" />
      </div>

      {/* Timeline renderer */}
      {layout ? (
        <div
          className={embedded ? "pb-3" : "px-2 pb-3"}
          style={{
            minHeight: layout.totalHeight + 8,
          }}
        >
          <TimelineRenderer
            layout={layout}
            tasks={tasks}
            config={layoutConfig}
            onTaskClick={handleTaskClick}
          />
        </div>
      ) : (
        <div className="flex items-center justify-center py-8">
          <Loader2 className="h-4 w-4 animate-spin text-gray-300" />
          <span className="ml-2 text-xs text-gray-400">Computing layout…</span>
        </div>
      )}

      {/* ---- Expanded modal ---- */}
      {isModalOpen && (
        <TimelineModal
          isOpen
          onClose={() => setIsModalOpen(false)}
          tasks={tasks}
          taskEdges={taskEdges}
          milestones={milestones}
          milestoneEdges={milestoneEdges}
          suppressedEdgeKeys={suppressedEdgeKeys}
          onTaskClick={handleTaskClick}
          summary={summary}
        />
      )}
    </>
  );

  // ---- Embedded mode: no card, no header, just the content ----
  if (embedded) {
    return (
      <div ref={containerRef} className="pt-1">
        {timelineContent}
      </div>
    );
  }

  // ---- Standalone mode: full card with header + collapse ----
  return (
    <div className="bg-white shadow rounded-lg" ref={containerRef}>
      {/* ---- Header ---- */}
      <button
        onClick={() => setIsCollapsed(!isCollapsed)}
        className="w-full flex items-center justify-between px-5 py-3 text-left hover:bg-gray-50 rounded-t-lg transition-colors"
      >
        <div className="flex items-center gap-2.5">
          {isCollapsed ? (
            <ChevronRight className="h-4 w-4 text-gray-400" />
          ) : (
            <ChevronDown className="h-4 w-4 text-gray-400" />
          )}
          <ChartGantt className="h-4 w-4 text-indigo-500" />
          <h3 className="text-sm font-semibold text-gray-800">
            Workflow Timeline
          </h3>
          <span className="text-xs text-gray-400">
            {summary.total} task{summary.total !== 1 ? "s" : ""}
            {summary.durationMs != null && (
              <> · {formatDurationShort(summary.durationMs)}</>
            )}
          </span>
        </div>

        {/* Summary badges */}
        <div className="flex items-center gap-1.5">
          {summary.completed > 0 && (
            <span className="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-green-100 text-green-700">
              {summary.completed} ✓
            </span>
          )}
          {summary.running > 0 && (
            <span className="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-blue-100 text-blue-700">
              {summary.running} ⟳
            </span>
          )}
          {summary.failed > 0 && (
            <span className="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-red-100 text-red-700">
              {summary.failed} ✗
            </span>
          )}
          {summary.other > 0 && (
            <span className="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-gray-100 text-gray-500">
              {summary.other}
            </span>
          )}
        </div>
      </button>

      {/* ---- Body ---- */}
      {!isCollapsed && (
        <div className="border-t border-gray-100">{timelineContent}</div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Legend sub-components
// ---------------------------------------------------------------------------

function LegendItem({
  color,
  label,
  dashed,
  dotted,
}: {
  color: string;
  label: string;
  dashed?: boolean;
  dotted?: boolean;
}) {
  return (
    <span className="flex items-center gap-1">
      <span
        className="inline-block w-5 h-2.5 rounded-sm"
        style={{
          backgroundColor: color,
          opacity: 0.7,
          border: dashed
            ? `1px dashed ${color}`
            : dotted
              ? `1px dotted ${color}`
              : undefined,
        }}
      />
      <span>{label}</span>
    </span>
  );
}

function EdgeLegendItem({
  color,
  label,
  dashed,
}: {
  color: string;
  label: string;
  dashed?: boolean;
}) {
  return (
    <span className="flex items-center gap-1">
      <svg width="16" height="8" viewBox="0 0 16 8">
        <line
          x1="0"
          y1="4"
          x2="16"
          y2="4"
          stroke={color}
          strokeWidth="1.5"
          strokeDasharray={dashed ? "3 2" : undefined}
          opacity="0.7"
        />
        <polygon points="12,1 16,4 12,7" fill={color} opacity="0.6" />
      </svg>
      <span>{label}</span>
    </span>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatDurationShort(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const secs = ms / 1000;
  if (secs < 60) return `${secs.toFixed(1)}s`;
  const mins = Math.floor(secs / 60);
  const remainSecs = Math.round(secs % 60);
  if (mins < 60) return `${mins}m ${remainSecs}s`;
  const hrs = Math.floor(mins / 60);
  const remainMins = mins % 60;
  return `${hrs}h ${remainMins}m`;
}
