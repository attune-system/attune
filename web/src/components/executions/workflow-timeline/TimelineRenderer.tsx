/**
 * TimelineRenderer — Pure SVG renderer for the Workflow Timeline DAG.
 *
 * Renders:
 *   - Vertical gridlines with time labels along the top
 *   - Task bars (horizontal rounded rectangles) colored by state
 *   - Milestone nodes (small diamonds)
 *   - Curved Bezier dependency edges with transition-aware coloring/labels
 *   - Hover tooltips with task details
 *   - Click-to-select with upstream/downstream path highlighting
 */

import { useState, useRef, useCallback, useMemo, useEffect } from "react";
import type {
  ComputedLayout,
  TimelineNode,
  TimelineEdge,
  TimelineTask,
  TooltipData,
  LayoutConfig,
  EdgeKind,
  WithItemsGroupInfo,
} from "./types";
import {
  STATE_COLORS,
  EDGE_KIND_COLORS,
  MILESTONE_COLORS,
  DEFAULT_LAYOUT,
} from "./types";
import { computeEdgePath, computeGridLines, type GridLine } from "./layout";
import { findConnectedPath, edgeKey } from "./data";

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface TimelineRendererProps {
  layout: ComputedLayout;
  tasks: TimelineTask[];
  config?: LayoutConfig;
  /** Callback when a task bar is clicked (e.g. navigate to execution detail) */
  onTaskClick?: (task: TimelineTask) => void;
  /** Prefix for SVG element IDs to avoid collisions when multiple renderers coexist */
  idPrefix?: string;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatDuration(ms: number): string {
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

function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function stateLabel(state: string): string {
  return state.charAt(0).toUpperCase() + state.slice(1);
}

function edgeColor(edge: TimelineEdge): string {
  if (edge.color) return edge.color;
  return EDGE_KIND_COLORS[edge.kind] ?? EDGE_KIND_COLORS.always;
}

function edgeOpacity(
  edge: TimelineEdge,
  isHighlighted: boolean,
  hasSelection: boolean,
): number {
  if (!hasSelection) {
    // No selection — show all edges at moderate opacity
    return edge.kind === "failure" || edge.kind === "timeout" ? 0.45 : 0.35;
  }
  return isHighlighted ? 0.85 : 0.08;
}

function edgeWidth(
  edge: TimelineEdge,
  isHighlighted: boolean,
  hasSelection: boolean,
): number {
  if (hasSelection && isHighlighted) return 2;
  if (edge.kind === "failure" || edge.kind === "timeout") return 1.5;
  return 1.2;
}

/** Dash array for failure/timeout edges */
function edgeDash(kind: EdgeKind): string | undefined {
  if (kind === "failure") return "4 3";
  if (kind === "timeout") return "6 3 2 3";
  return undefined;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function TimelineRenderer({
  layout,
  tasks,
  config: configOverride,
  onTaskClick,
  idPrefix = "",
}: TimelineRendererProps) {
  const config = useMemo<LayoutConfig>(
    () => ({ ...DEFAULT_LAYOUT, ...configOverride }),
    [configOverride],
  );

  const containerRef = useRef<HTMLDivElement>(null);

  // Track container width in state so the tooltip can use it without
  // reading a ref during render.
  const [containerWidth, setContainerWidth] = useState(800);
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    setContainerWidth(el.clientWidth);
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        if (entry.contentRect.width > 0) {
          setContainerWidth(entry.contentRect.width);
        }
      }
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // ---- Interaction state ----
  const [tooltip, setTooltip] = useState<TooltipData | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);

  // ---- Node lookup ----
  const nodeMap = useMemo(() => {
    const map = new Map<string, TimelineNode>();
    for (const n of layout.nodes) {
      map.set(n.id, n);
    }
    return map;
  }, [layout.nodes]);

  // ---- Highlighted path ----
  const highlighted = useMemo(() => {
    if (!selectedTaskId) return null;
    return findConnectedPath(selectedTaskId, tasks, layout.edges);
  }, [selectedTaskId, tasks, layout.edges]);

  // ---- Grid lines ----
  const gridLines = useMemo<GridLine[]>(() => {
    // Reconstruct the time scale from layout bounds
    const axisWidth =
      layout.totalWidth - config.paddingLeft - config.paddingRight;
    const pxPerMs =
      axisWidth / Math.max(layout.maxTimeMs - layout.minTimeMs, 1);
    return computeGridLines(
      {
        minMs: layout.minTimeMs,
        maxMs: layout.maxTimeMs,
        axisWidth,
        pxPerMs,
      },
      config,
    );
  }, [layout.totalWidth, layout.minTimeMs, layout.maxTimeMs, config]);

  // ---- Computed SVG dimensions ----
  const svgWidth = layout.totalWidth;
  const svgHeight = layout.totalHeight;

  // ---- Edge paths (memoised) ----
  const edgePaths = useMemo(() => {
    return layout.edges
      .map((edge) => {
        const fromNode = nodeMap.get(edge.from);
        const toNode = nodeMap.get(edge.to);
        if (!fromNode || !toNode) return null;
        const path = computeEdgePath(fromNode, toNode, config);
        return { edge, path, fromNode, toNode };
      })
      .filter((p): p is NonNullable<typeof p> => p != null);
  }, [layout.edges, nodeMap, config]);

  // ---- Handlers ----
  const handleTaskHover = useCallback(
    (task: TimelineTask, e: React.MouseEvent) => {
      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;
      setTooltip({
        task,
        x: e.clientX - rect.left,
        y: e.clientY - rect.top,
      });
    },
    [],
  );

  const handleTaskLeave = useCallback(() => {
    setTooltip(null);
  }, []);

  const handleTaskClick = useCallback(
    (task: TimelineTask, e: React.MouseEvent) => {
      e.stopPropagation();
      if (selectedTaskId === task.id) {
        setSelectedTaskId(null); // deselect
      } else {
        setSelectedTaskId(task.id);
      }
    },
    [selectedTaskId],
  );

  const handleBackgroundClick = useCallback(() => {
    setSelectedTaskId(null);
  }, []);

  // ---- Determine if anything is selected ----
  const hasSelection = selectedTaskId != null;

  // ---- Render ----
  return (
    <div className="relative select-none">
      {/* Hint */}
      <div className="absolute bottom-1 right-2 z-10 text-[10px] text-gray-400 pointer-events-none">
        Click task to highlight path · Double-click to view details
      </div>

      {/* Scrollable container */}
      <div ref={containerRef} className="overflow-x-auto overflow-y-hidden">
        <svg
          width={svgWidth}
          height={svgHeight}
          viewBox={`0 0 ${layout.totalWidth} ${layout.totalHeight}`}
          className="block"
          style={{
            width: svgWidth,
            height: svgHeight,
          }}
          onClick={handleBackgroundClick}
        >
          {/* ---- Definitions ---- */}
          <defs>
            {/* Subtle drop shadow for task bars */}
            <filter
              id={`${idPrefix}barShadow`}
              x="-2%"
              y="-10%"
              width="104%"
              height="130%"
            >
              <feDropShadow
                dx="0"
                dy="1"
                stdDeviation="1.5"
                floodOpacity="0.08"
              />
            </filter>
            {/* Arrowhead markers for each edge kind */}
            {(
              [
                "success",
                "failure",
                "always",
                "timeout",
                "custom",
              ] as EdgeKind[]
            ).map((kind) => (
              <marker
                key={kind}
                id={`${idPrefix}arrow-${kind}`}
                viewBox="0 0 10 10"
                refX="9"
                refY="5"
                markerWidth="6"
                markerHeight="6"
                orient="auto-start-reverse"
              >
                <path
                  d="M 0 1 L 10 5 L 0 9 z"
                  fill={EDGE_KIND_COLORS[kind]}
                  opacity="0.6"
                />
              </marker>
            ))}
            {/* Custom-colored arrow for edges with explicit color */}
            <marker
              id={`${idPrefix}arrow-custom-color`}
              viewBox="0 0 10 10"
              refX="9"
              refY="5"
              markerWidth="6"
              markerHeight="6"
              orient="auto-start-reverse"
            >
              <path
                d="M 0 1 L 10 5 L 0 9 z"
                fill="currentColor"
                opacity="0.6"
              />
            </marker>
          </defs>

          {/* ---- Background ---- */}
          <rect
            width={layout.totalWidth}
            height={layout.totalHeight}
            fill="#fafbfc"
            rx="0"
          />

          {/* ---- Lane stripes (alternating very subtle shading) ---- */}
          {Array.from({ length: layout.laneCount }, (_, i) => (
            <rect
              key={`lane-${i}`}
              x={0}
              y={config.paddingTop + i * config.laneHeight}
              width={layout.totalWidth}
              height={config.laneHeight}
              fill={i % 2 === 0 ? "transparent" : "rgba(0,0,0,0.015)"}
            />
          ))}

          {/* ---- Grid lines ---- */}
          {gridLines.map((gl, i) => (
            <g key={`grid-${i}`}>
              <line
                x1={gl.x}
                y1={config.paddingTop}
                x2={gl.x}
                y2={layout.totalHeight}
                stroke={gl.major ? "#e5e7eb" : "#f3f4f6"}
                strokeWidth={gl.major ? 1 : 0.5}
                strokeDasharray={gl.major ? undefined : "2 4"}
              />
              {gl.major && gl.label && (
                <text
                  x={gl.x}
                  y={config.paddingTop - 8}
                  textAnchor="middle"
                  fill="#9ca3af"
                  fontSize="10"
                  fontFamily="ui-monospace, SFMono-Regular, Menlo, monospace"
                >
                  {gl.label}
                </text>
              )}
            </g>
          ))}

          {/* ---- Edges (rendered behind nodes) ---- */}
          <g className="edges">
            {edgePaths.map(({ edge, path }, i) => {
              const key = edgeKey(edge.from, edge.to);
              const isHl = highlighted?.edgeKeys.has(key) ?? false;
              const color = edgeColor(edge);
              const opacity = edgeOpacity(edge, isHl, hasSelection);
              const width = edgeWidth(edge, isHl, hasSelection);
              const dash = edgeDash(edge.kind);

              return (
                <g key={`edge-${i}`}>
                  <path
                    d={path}
                    fill="none"
                    stroke={color}
                    strokeWidth={width}
                    strokeOpacity={opacity}
                    strokeDasharray={dash}
                    markerEnd={
                      edge.color
                        ? undefined // custom-color arrows are hard via markers; skip
                        : `url(#${idPrefix}arrow-${edge.kind})`
                    }
                    style={{
                      transition:
                        "stroke-opacity 0.2s ease, stroke-width 0.15s ease",
                    }}
                  />
                  {/* Edge label */}
                  {edge.label && (
                    <EdgeLabel
                      path={path}
                      label={edge.label}
                      color={color}
                      opacity={hasSelection ? (isHl ? 0.9 : 0.1) : 0.6}
                    />
                  )}
                </g>
              );
            })}
          </g>

          {/* ---- Milestone nodes ---- */}
          {layout.nodes
            .filter((n) => n.type === "milestone" && n.milestone)
            .map((node) => {
              const ms = node.milestone!;
              const size = config.milestoneSize;
              const color = MILESTONE_COLORS[ms.kind];
              const isHl = highlighted?.nodeIds.has(node.id) ?? false;
              const nodeOpacity = hasSelection ? (isHl ? 1 : 0.15) : 1;

              return (
                <g
                  key={node.id}
                  transform={`translate(${node.x}, ${node.y})`}
                  opacity={nodeOpacity}
                  style={{ transition: "opacity 0.2s ease" }}
                >
                  {/* Diamond shape */}
                  <rect
                    x={-size / 2}
                    y={-size / 2}
                    width={size}
                    height={size}
                    rx={2}
                    fill={color}
                    transform="rotate(45)"
                    stroke="white"
                    strokeWidth={1.5}
                  />
                  {/* Label */}
                  {ms.label && (
                    <text
                      y={size / 2 + 12}
                      textAnchor="middle"
                      fill="#6b7280"
                      fontSize="9"
                      fontWeight="500"
                    >
                      {ms.label}
                    </text>
                  )}
                </g>
              );
            })}

          {/* ---- Task bars ---- */}
          {layout.nodes
            .filter((n) => n.type === "task" && n.task)
            .map((node) => {
              const task = node.task!;
              const gi = task.groupInfo;
              const colors = STATE_COLORS[task.state];
              const isSelected = selectedTaskId === task.id;
              const isHl = highlighted?.nodeIds.has(node.id) ?? false;
              const nodeOpacity = hasSelection ? (isHl ? 1 : 0.18) : 1;
              const barRadius = 4;

              // Running tasks (or groups with running items) get a subtle pulse
              const isRunning =
                task.state === "running" || task.state === "waiting";

              return (
                <g
                  key={node.id}
                  className="cursor-pointer"
                  opacity={nodeOpacity}
                  style={{ transition: "opacity 0.2s ease" }}
                  onMouseEnter={(e) => handleTaskHover(task, e)}
                  onMouseMove={(e) => handleTaskHover(task, e)}
                  onMouseLeave={handleTaskLeave}
                  onClick={(e) => handleTaskClick(task, e)}
                  onDoubleClick={(e) => {
                    e.stopPropagation();
                    onTaskClick?.(task);
                  }}
                >
                  {/* Selection ring */}
                  {isSelected && (
                    <rect
                      x={node.x - 2}
                      y={node.y - 2}
                      width={node.width + 4}
                      height={config.barHeight + 4}
                      rx={barRadius + 1}
                      fill="none"
                      stroke={colors.border}
                      strokeWidth={2}
                      opacity={0.6}
                    />
                  )}

                  {/* Bar background */}
                  <rect
                    x={node.x}
                    y={node.y}
                    width={node.width}
                    height={config.barHeight}
                    rx={barRadius}
                    fill={colors.bg}
                    stroke={colors.border}
                    strokeWidth={isSelected ? 1.5 : 1}
                    strokeDasharray={gi ? "3 2" : undefined}
                    filter={`url(#${idPrefix}barShadow)`}
                  >
                    {isRunning && (
                      <animate
                        attributeName="opacity"
                        values="1;0.75;1"
                        dur="2s"
                        repeatCount="indefinite"
                      />
                    )}
                  </rect>

                  {/* Group progress segments — stacked colored bar showing
                      completed / running / failed / pending proportions */}
                  {gi && gi.totalItems > 0 && (
                    <GroupProgressBar
                      x={node.x}
                      y={node.y}
                      width={node.width}
                      height={config.barHeight}
                      radius={barRadius}
                      groupInfo={gi}
                      isRunning={isRunning}
                    />
                  )}

                  {/* Progress fill for running tasks (non-group only) */}
                  {!gi && isRunning && task.startMs != null && (
                    <rect
                      x={node.x}
                      y={node.y}
                      width={node.width}
                      height={config.barHeight}
                      rx={barRadius}
                      fill={colors.border}
                      opacity={0.12}
                    >
                      <animate
                        attributeName="opacity"
                        values="0.06;0.15;0.06"
                        dur="2s"
                        repeatCount="indefinite"
                      />
                    </rect>
                  )}

                  {/* Left accent bar for state */}
                  <rect
                    x={node.x}
                    y={node.y}
                    width={3}
                    height={config.barHeight}
                    rx={barRadius}
                    fill={colors.border}
                  />

                  {/* Task name label */}
                  <clipPath id={`${idPrefix}clip-${node.id}`}>
                    <rect
                      x={node.x + 6}
                      y={node.y}
                      width={Math.max(0, node.width - 12)}
                      height={config.barHeight}
                    />
                  </clipPath>
                  <text
                    x={node.x + 8}
                    y={node.y + config.barHeight / 2}
                    dominantBaseline="central"
                    fill={colors.text}
                    fontSize="11"
                    fontWeight="500"
                    fontFamily="ui-sans-serif, system-ui, -apple-system, sans-serif"
                    clipPath={`url(#${idPrefix}clip-${node.id})`}
                  >
                    {task.name}
                    {task.taskIndex != null ? ` [${task.taskIndex}]` : ""}
                    {gi && (
                      <tspan
                        fill={colors.border}
                        fontSize="10"
                        fontWeight="600"
                      >
                        {" — "}
                        {gi.completed + gi.failed + gi.timedOut + gi.cancelled}
                        {" of "}
                        {gi.totalItems}
                        {" completed"}
                      </tspan>
                    )}
                  </text>

                  {/* Timed-out indicator */}
                  {task.timedOut && (
                    <g
                      transform={`translate(${node.x + node.width - 14}, ${node.y + 3})`}
                    >
                      <circle
                        r="5"
                        fill="#f97316"
                        opacity="0.9"
                        cx="5"
                        cy="5"
                      />
                      <text
                        x="5"
                        y="5.5"
                        textAnchor="middle"
                        dominantBaseline="central"
                        fill="white"
                        fontSize="7"
                        fontWeight="bold"
                      >
                        !
                      </text>
                    </g>
                  )}

                  {/* Terminal indicator — right-side end-cap for leaf tasks */}
                  {task.downstreamIds.length === 0 &&
                    task.state !== "running" &&
                    task.state !== "waiting" &&
                    task.state !== "pending" && (
                      <rect
                        x={node.x + node.width - 3}
                        y={node.y}
                        width={3}
                        height={config.barHeight}
                        rx={1}
                        fill={colors.border}
                        opacity={0.6}
                      />
                    )}
                </g>
              );
            })}
        </svg>
      </div>

      {/* ---- Tooltip ---- */}
      {tooltip && (
        <TaskTooltip tooltip={tooltip} containerWidth={containerWidth} />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Edge label sub-component
// ---------------------------------------------------------------------------

function EdgeLabel({
  path,
  label,
  color,
  opacity,
}: {
  path: string;
  label: string;
  color: string;
  opacity: number;
}) {
  // Parse the path to approximate the midpoint of a cubic Bezier
  // M x0 y0 C cx1 cy1, cx2 cy2, x1 y1
  const mid = useMemo(() => {
    const nums = path.match(/-?[\d.]+/g)?.map(Number);
    if (!nums || nums.length < 8) return null;

    const [x0, y0, cx1, cy1, cx2, cy2, x1, y1] = nums;
    // Cubic Bezier at t=0.5
    const t = 0.45; // slightly before midpoint to avoid overlap with arrowhead
    const mt = 1 - t;
    const mx =
      mt * mt * mt * x0 +
      3 * mt * mt * t * cx1 +
      3 * mt * t * t * cx2 +
      t * t * t * x1;
    const my =
      mt * mt * mt * y0 +
      3 * mt * mt * t * cy1 +
      3 * mt * t * t * cy2 +
      t * t * t * y1;

    return { x: mx, y: my };
  }, [path]);

  if (!mid) return null;

  return (
    <g opacity={opacity} style={{ transition: "opacity 0.2s ease" }}>
      {/* Background pill */}
      <rect
        x={mid.x - label.length * 2.8 - 4}
        y={mid.y - 7}
        width={label.length * 5.6 + 8}
        height={14}
        rx={3}
        fill="white"
        stroke={color}
        strokeWidth={0.5}
        opacity={0.92}
      />
      <text
        x={mid.x}
        y={mid.y}
        textAnchor="middle"
        dominantBaseline="central"
        fill={color}
        fontSize="9"
        fontWeight="600"
        fontFamily="ui-sans-serif, system-ui, sans-serif"
      >
        {label}
      </text>
    </g>
  );
}

// ---------------------------------------------------------------------------
// Tooltip sub-component
// ---------------------------------------------------------------------------

function TaskTooltip({
  tooltip,
  containerWidth: cw,
}: {
  tooltip: TooltipData;
  containerWidth: number;
}) {
  const { task, x, y } = tooltip;
  const gi = task.groupInfo;

  // Position the tooltip to avoid clipping at the edges
  const tooltipWidth = 280;
  const left = x + tooltipWidth + 16 > cw ? x - tooltipWidth - 8 : x + 16;
  const top = Math.max(8, y - 10);

  return (
    <div className="absolute z-30 pointer-events-none" style={{ left, top }}>
      <div
        className="bg-gray-900 text-white rounded-lg shadow-xl px-3.5 py-3 text-xs"
        style={{ width: tooltipWidth }}
      >
        {/* Header */}
        <div className="flex items-center gap-2 mb-2">
          <span
            className="inline-block w-2.5 h-2.5 rounded-sm flex-shrink-0"
            style={{ backgroundColor: STATE_COLORS[task.state].border }}
          />
          <span className="font-semibold text-sm truncate">{task.name}</span>
          {task.taskIndex != null && (
            <span className="text-gray-400 text-[10px]">
              [{task.taskIndex}]
            </span>
          )}
        </div>

        {/* Group progress summary (collapsed with_items) */}
        {gi && (
          <div className="mb-2 space-y-1.5">
            {/* Stacked progress bar */}
            <div className="flex h-2 rounded-full overflow-hidden bg-gray-700">
              {gi.completed > 0 && (
                <div
                  className="bg-green-500"
                  style={{
                    width: `${(gi.completed / gi.totalItems) * 100}%`,
                  }}
                />
              )}
              {gi.running > 0 && (
                <div
                  className="bg-blue-500 animate-pulse"
                  style={{
                    width: `${(gi.running / gi.totalItems) * 100}%`,
                  }}
                />
              )}
              {gi.failed > 0 && (
                <div
                  className="bg-red-500"
                  style={{
                    width: `${(gi.failed / gi.totalItems) * 100}%`,
                  }}
                />
              )}
              {gi.timedOut > 0 && (
                <div
                  className="bg-orange-500"
                  style={{
                    width: `${(gi.timedOut / gi.totalItems) * 100}%`,
                  }}
                />
              )}
            </div>
            {/* Counts row */}
            <div className="flex gap-2 text-[10px] text-gray-400 flex-wrap">
              {gi.completed > 0 && (
                <span className="text-green-400">✓ {gi.completed}</span>
              )}
              {gi.running > 0 && (
                <span className="text-blue-400">⟳ {gi.running}</span>
              )}
              {gi.pending > 0 && (
                <span className="text-gray-400">○ {gi.pending}</span>
              )}
              {gi.failed > 0 && (
                <span className="text-red-400">✗ {gi.failed}</span>
              )}
              {gi.timedOut > 0 && (
                <span className="text-orange-400">⏱ {gi.timedOut}</span>
              )}
              {gi.cancelled > 0 && (
                <span className="text-gray-500">⊘ {gi.cancelled}</span>
              )}
              <span className="text-gray-500 ml-auto">
                of {gi.totalItems} items
              </span>
            </div>
            {gi.concurrency > 0 && (
              <Row label="Concurrency" value={`${gi.concurrency}`} />
            )}
          </div>
        )}

        {/* Details grid */}
        <div className="space-y-1 text-gray-300">
          <Row label="State" value={stateLabel(task.state)} />
          <Row label="Action" value={task.actionRef} />
          {task.startMs != null && (
            <Row label="Started" value={formatTime(task.startMs)} />
          )}
          {task.endMs != null && (
            <Row label="Ended" value={formatTime(task.endMs)} />
          )}
          {task.durationMs != null && task.durationMs > 0 && (
            <Row label="Duration" value={formatDuration(task.durationMs)} />
          )}
          {!gi && task.timedOut && (
            <Row label="Timed Out" value="Yes" valueClass="text-orange-400" />
          )}
          {!gi && task.maxRetries > 0 && (
            <Row
              label="Retries"
              value={`${task.retryCount} / ${task.maxRetries}`}
            />
          )}
          {task.upstreamIds.length > 0 && (
            <Row
              label="Upstream"
              value={`${task.upstreamIds.length} task${task.upstreamIds.length !== 1 ? "s" : ""}`}
            />
          )}
          {task.downstreamIds.length > 0 && (
            <Row
              label="Downstream"
              value={`${task.downstreamIds.length} task${task.downstreamIds.length !== 1 ? "s" : ""}`}
            />
          )}
          {task.downstreamIds.length === 0 &&
            task.state !== "running" &&
            task.state !== "waiting" &&
            task.state !== "pending" && (
              <Row
                label="Terminal"
                value="No further tasks"
                valueClass="text-gray-500 italic"
              />
            )}
        </div>

        {/* Footer hint */}
        <div className="mt-2 pt-1.5 border-t border-gray-700 text-[10px] text-gray-500">
          {gi
            ? "Click to highlight path"
            : "Click to highlight path · Double-click to view details"}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// GroupProgressBar — stacked segment fill for collapsed with_items nodes
// ---------------------------------------------------------------------------

function GroupProgressBar({
  x,
  y,
  width,
  height,
  radius,
  groupInfo: gi,
  isRunning,
}: {
  x: number;
  y: number;
  width: number;
  height: number;
  radius: number;
  groupInfo: WithItemsGroupInfo;
  isRunning: boolean;
}) {
  const total = gi.totalItems;
  if (total === 0) return null;

  // Compute segment widths as proportions of the bar
  const innerWidth = width;
  const segments: { color: string; w: number; animate?: boolean }[] = [];

  if (gi.completed > 0)
    segments.push({
      color: STATE_COLORS.completed.border,
      w: (gi.completed / total) * innerWidth,
    });
  if (gi.running > 0)
    segments.push({
      color: STATE_COLORS.running.border,
      w: (gi.running / total) * innerWidth,
      animate: true,
    });
  if (gi.failed > 0)
    segments.push({
      color: STATE_COLORS.failed.border,
      w: (gi.failed / total) * innerWidth,
    });
  if (gi.timedOut > 0)
    segments.push({
      color: "#f97316",
      w: (gi.timedOut / total) * innerWidth,
    });
  if (gi.cancelled > 0)
    segments.push({
      color: "#9ca3af",
      w: (gi.cancelled / total) * innerWidth,
    });

  let offsetX = 0;

  return (
    <g>
      {/* Clip path to keep segments within the rounded bar */}
      <clipPath id={`group-progress-${gi.memberIds[0]}`}>
        <rect x={x} y={y} width={width} height={height} rx={radius} />
      </clipPath>
      <g clipPath={`url(#group-progress-${gi.memberIds[0]})`}>
        {segments.map((seg, i) => {
          const sx = x + offsetX;
          offsetX += seg.w;
          return (
            <rect
              key={i}
              x={sx}
              y={y}
              width={Math.max(seg.w, 1)}
              height={height}
              fill={seg.color}
              opacity={0.25}
            >
              {seg.animate && (
                <animate
                  attributeName="opacity"
                  values="0.15;0.35;0.15"
                  dur="2s"
                  repeatCount="indefinite"
                />
              )}
            </rect>
          );
        })}
      </g>
      {/* Thin progress track at the bottom of the bar */}
      <g clipPath={`url(#group-progress-${gi.memberIds[0]})`}>
        {(() => {
          let bx = 0;
          return segments.map((seg, i) => {
            const sx = x + bx;
            bx += seg.w;
            return (
              <rect
                key={`b${i}`}
                x={sx}
                y={y + height - 3}
                width={Math.max(seg.w, 1)}
                height={3}
                fill={seg.color}
                opacity={0.7}
              >
                {seg.animate && (
                  <animate
                    attributeName="opacity"
                    values="0.5;0.9;0.5"
                    dur="2s"
                    repeatCount="indefinite"
                  />
                )}
              </rect>
            );
          });
        })()}
      </g>
      {/* Subtle overall pulse when still running */}
      {isRunning && (
        <rect
          x={x}
          y={y}
          width={width}
          height={height}
          rx={radius}
          fill={STATE_COLORS.running.border}
          opacity={0.06}
        >
          <animate
            attributeName="opacity"
            values="0.03;0.08;0.03"
            dur="2s"
            repeatCount="indefinite"
          />
        </rect>
      )}
    </g>
  );
}

// ---------------------------------------------------------------------------
// Row / TaskTooltip sub-components
// ---------------------------------------------------------------------------

function Row({
  label,
  value,
  valueClass,
}: {
  label: string;
  value: string;
  valueClass?: string;
}) {
  return (
    <div className="flex justify-between gap-3">
      <span className="text-gray-400 flex-shrink-0">{label}</span>
      <span className={`truncate text-right ${valueClass ?? "text-gray-200"}`}>
        {value}
      </span>
    </div>
  );
}
