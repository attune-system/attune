/**
 * Workflow Timeline DAG Types
 *
 * Types for the Prefect-style workflow run timeline visualization.
 * This component renders workflow task executions as horizontal duration bars
 * on a time axis with curved dependency edges showing the DAG structure.
 */

// ---------------------------------------------------------------------------
// Core data types
// ---------------------------------------------------------------------------

export type TaskState =
  | "completed"
  | "running"
  | "failed"
  | "pending"
  | "timeout"
  | "cancelled"
  | "abandoned"
  | "waiting"
  | "released";

export type TimelineTaskDestination =
  | { kind: "execution"; executionId: number }
  | { kind: "inquiry"; inquiryId: number };

/**
 * Metadata for a collapsed with_items group node.
 * When a with_items task has ≥ WITH_ITEMS_COLLAPSE_THRESHOLD items, all
 * individual item executions are merged into a single TimelineTask carrying
 * this info so the renderer can display a compact "task ×N" bar.
 */
export interface WithItemsGroupInfo {
  /** Total number of items in the group */
  totalItems: number;
  /** Per-state item counts */
  completed: number;
  failed: number;
  running: number;
  pending: number;
  timedOut: number;
  cancelled: number;
  /** Concurrency limit declared on the task (0 = unlimited / unknown) */
  concurrency: number;
  /** IDs of all member executions (for upstream/downstream tracking) */
  memberIds: string[];
}

/** Threshold at which with_items children are collapsed into a single node */
export const WITH_ITEMS_COLLAPSE_THRESHOLD = 10;

/** A single task run positioned on the timeline */
export interface TimelineTask {
  /** Unique identifier (execution ID as string) */
  id: string;
  /** Display name (task_name from workflow_task metadata) */
  name: string;
  /** Action reference */
  actionRef: string;
  /** Visual state for coloring */
  state: TaskState;
  /** Start time as epoch ms (null if not yet started) */
  startMs: number | null;
  /** End time as epoch ms (null if still running or not started) */
  endMs: number | null;
  /** IDs of upstream tasks this depends on */
  upstreamIds: string[];
  /** IDs of downstream tasks that depend on this */
  downstreamIds: string[];
  /** with_items task index (null if not a with_items expansion) */
  taskIndex: number | null;
  /** Whether this task timed out */
  timedOut: boolean;
  /** Retry info */
  retryCount: number;
  maxRetries: number;
  /** Duration in ms (from metadata or computed) */
  durationMs: number | null;
  /** Detail page opened from the task */
  destination: TimelineTaskDestination;
  /**
   * Present only on collapsed with_items group nodes.
   * When set, this task represents multiple item executions merged into one.
   */
  groupInfo?: WithItemsGroupInfo;
}

// ---------------------------------------------------------------------------
// Synthetic milestone / junction nodes
// ---------------------------------------------------------------------------

export type MilestoneKind = "start" | "end" | "merge" | "fork";

export interface TimelineMilestone {
  id: string;
  kind: MilestoneKind;
  /** Position on the time axis (epoch ms) */
  timeMs: number;
  /** Human-readable label */
  label: string;
}

// ---------------------------------------------------------------------------
// Unified node type (task bar OR milestone)
// ---------------------------------------------------------------------------

export type TimelineNodeType = "task" | "milestone";

export interface TimelineNode {
  type: TimelineNodeType;
  /** Unique ID */
  id: string;
  /** Assigned lane (y index) */
  lane: number;
  /** Pixel positions (computed by layout) */
  x: number;
  y: number;
  width: number;
  /** Original data */
  task?: TimelineTask;
  milestone?: TimelineMilestone;
}

// ---------------------------------------------------------------------------
// Edges
// ---------------------------------------------------------------------------

export type EdgeKind = "success" | "failure" | "always" | "timeout" | "custom";

export interface TimelineEdge {
  /** Source node ID */
  from: string;
  /** Target node ID */
  to: string;
  /** Visual classification for coloring */
  kind: EdgeKind;
  /** Optional transition label (e.g. "succeeded", "failed") */
  label?: string;
  /** Optional custom color from workflow definition */
  color?: string;
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

export interface LayoutConfig {
  /** Height of each lane in pixels */
  laneHeight: number;
  /** Height of a task bar in pixels */
  barHeight: number;
  /** Vertical padding within each lane */
  lanePadding: number;
  /** Size of milestone diamond/square in pixels */
  milestoneSize: number;
  /** Left padding for the chart area (px) */
  paddingLeft: number;
  /** Right padding for the chart area (px) */
  paddingRight: number;
  /** Top padding for the time axis area (px) */
  paddingTop: number;
  /** Bottom padding (px) */
  paddingBottom: number;
  /** Minimum bar width for very short tasks (px) */
  minBarWidth: number;
  /** Horizontal gap between milestone and adjacent bars (px) */
  milestoneGap: number;
}

export const DEFAULT_LAYOUT: LayoutConfig = {
  laneHeight: 32,
  barHeight: 20,
  lanePadding: 6,
  milestoneSize: 10,
  paddingLeft: 20,
  paddingRight: 20,
  paddingTop: 36,
  paddingBottom: 16,
  minBarWidth: 8,
  milestoneGap: 12,
};

// ---------------------------------------------------------------------------
// Computed layout result
// ---------------------------------------------------------------------------

export interface ComputedLayout {
  nodes: TimelineNode[];
  edges: TimelineEdge[];
  /** Total width needed (px) */
  totalWidth: number;
  /** Total height needed (px) */
  totalHeight: number;
  /** Number of lanes used */
  laneCount: number;
  /** Time bounds */
  minTimeMs: number;
  maxTimeMs: number;
  /** The linear scale factor: px per ms */
  pxPerMs: number;
}

// ---------------------------------------------------------------------------
// Interaction state
// ---------------------------------------------------------------------------

export interface TooltipData {
  task: TimelineTask;
  x: number;
  y: number;
}

export interface ViewState {
  /** Horizontal scroll offset (px) */
  scrollX: number;
  /** Zoom level (1.0 = default) */
  zoom: number;
}

// ---------------------------------------------------------------------------
// Workflow definition transition types (for edge extraction)
// ---------------------------------------------------------------------------

export interface WorkflowDefinitionTransition {
  when?: string;
  publish?: Record<string, string>[];
  do?: string[];
  __chart_meta__?: {
    label?: string;
    color?: string;
    line_style?: string;
  };
}

export interface WorkflowDefinitionTask {
  name: string;
  action?: string;
  next?: WorkflowDefinitionTransition[];
  /** Number of inbound tasks that must complete before this task runs */
  join?: number;
  /** with_items expression (present when the task fans out over a list) */
  with_items?: string;
  /** Max concurrent items for with_items (default 1 = serial) */
  concurrency?: number;
  wait_for?: { inquiry: number | string };
  // Legacy fields (auto-converted to next)
  on_success?: string | string[];
  on_failure?: string | string[];
  on_complete?: string | string[];
  on_timeout?: string | string[];
}

export interface WorkflowDefinition {
  ref?: string;
  label?: string;
  tasks?: WorkflowDefinitionTask[];
}

// ---------------------------------------------------------------------------
// Color constants
// ---------------------------------------------------------------------------

export const STATE_COLORS: Record<
  TaskState,
  { bg: string; border: string; text: string }
> = {
  completed: { bg: "#dcfce7", border: "#22c55e", text: "#15803d" },
  running: { bg: "#dbeafe", border: "#3b82f6", text: "#1d4ed8" },
  failed: { bg: "#fee2e2", border: "#ef4444", text: "#b91c1c" },
  pending: { bg: "#f3f4f6", border: "#9ca3af", text: "#6b7280" },
  timeout: { bg: "#ffedd5", border: "#f97316", text: "#c2410c" },
  cancelled: { bg: "#f3f4f6", border: "#9ca3af", text: "#6b7280" },
  abandoned: { bg: "#fee2e2", border: "#f87171", text: "#b91c1c" },
  waiting: { bg: "#fef3c7", border: "#f59e0b", text: "#92400e" },
  released: { bg: "#ede9fe", border: "#8b5cf6", text: "#6d28d9" },
};

export const EDGE_KIND_COLORS: Record<EdgeKind, string> = {
  success: "#22c55e",
  failure: "#ef4444",
  always: "#9ca3af",
  timeout: "#f97316",
  custom: "#8b5cf6",
};

export const MILESTONE_COLORS: Record<MilestoneKind, string> = {
  start: "#6b7280",
  end: "#6b7280",
  merge: "#8b5cf6",
  fork: "#8b5cf6",
};
