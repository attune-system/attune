import { memo, useCallback, useRef, useState } from "react";
import { Trash2, GripVertical, Play, Octagon, Info } from "lucide-react";
import type { WorkflowTask, TransitionPreset } from "@/types/workflow";
import {
  PRESET_LABELS,
  PRESET_WHEN,
  classifyTransitionWhen,
} from "@/types/workflow";
import type { ScreenToCanvas } from "./WorkflowCanvas";
import PackIcon from "@/components/common/PackIcon";
import { packRefFromComponentRef } from "@/utils/packIcons";

export type { TransitionPreset };

interface TaskNodeProps {
  task: WorkflowTask;
  isSelected: boolean;
  isStartNode: boolean;
  allTaskNames: string[];
  onSelect: (taskId: string) => void;
  onDelete: (taskId: string) => void;
  onPositionChange: (
    taskId: string,
    position: { x: number; y: number },
  ) => void;
  onStartConnection: (taskId: string, preset: TransitionPreset) => void;
  connectingFrom: { taskId: string; preset: TransitionPreset } | null;
  onCompleteConnection: (targetTaskId: string) => void;
  /** Convert screen (client) coordinates to canvas-space coordinates. */
  screenToCanvas: ScreenToCanvas;
}

/** Handle visual configuration for each transition preset */
const HANDLE_CONFIG: {
  preset: TransitionPreset;
  color: string;
  hoverColor: string;
  activeColor: string;
  ringColor: string;
}[] = [
  {
    preset: "succeeded",
    color: "#22c55e",
    hoverColor: "#16a34a",
    activeColor: "#15803d",
    ringColor: "rgba(34, 197, 94, 0.3)",
  },
  {
    preset: "failed",
    color: "#ef4444",
    hoverColor: "#dc2626",
    activeColor: "#b91c1c",
    ringColor: "rgba(239, 68, 68, 0.3)",
  },
  {
    preset: "always",
    color: "#6b7280",
    hoverColor: "#4b5563",
    activeColor: "#374151",
    ringColor: "rgba(107, 114, 128, 0.3)",
  },
];

/**
 * Check if a task has an active transition matching a given preset.
 */
function hasActiveTransition(
  task: WorkflowTask,
  preset: TransitionPreset,
): boolean {
  if (!task.next) return false;
  const whenExpr = PRESET_WHEN[preset];
  return task.next.some((t) => {
    if (whenExpr === undefined) return t.when === undefined;
    return (
      t.when?.toLowerCase().replace(/\s+/g, "") ===
      whenExpr.toLowerCase().replace(/\s+/g, "")
    );
  });
}

/**
 * Compute a short summary of outgoing transitions for the tooltip.
 */
function transitionSummary(task: WorkflowTask): string | null {
  if (!task.next || task.next.length === 0) return null;
  const totalTargets = task.next.reduce(
    (sum, t) => sum + (t.do?.length ?? 0),
    0,
  );
  if (
    totalTargets === 0 &&
    task.next.some((t) => t.publish && t.publish.length > 0)
  ) {
    return `${task.next.length} transition${task.next.length !== 1 ? "s" : ""} (publish only)`;
  }
  if (totalTargets === 0) return null;
  return `${totalTargets} target${totalTargets !== 1 ? "s" : ""} via ${task.next.length} transition${task.next.length !== 1 ? "s" : ""}`;
}

/**
 * Check if a value is "populated" (non-null, non-undefined, non-empty-string).
 */
function hasValue(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value === "string" && value.trim() === "") return false;
  return true;
}

/**
 * Get entries from task.input that actually have values.
 */
function getPopulatedInputs(
  input: Record<string, unknown>,
): [string, unknown][] {
  return Object.entries(input).filter(([, v]) => hasValue(v));
}

/**
 * Format a value for inline display on the card — keep it short.
 */
function formatValueShort(value: unknown): string {
  if (typeof value === "string") {
    if (value.length > 28) return value.slice(0, 25) + "…";
    return value;
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (Array.isArray(value)) {
    return `[${value.length} items]`;
  }
  if (typeof value === "object" && value !== null) {
    return `{${Object.keys(value).length} keys}`;
  }
  return String(value);
}

/**
 * Format a value for the tooltip — can be slightly longer.
 */
function formatValueTooltip(value: unknown): string {
  if (typeof value === "string") {
    if (value.length > 40) return value.slice(0, 37) + "…";
    return value;
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (Array.isArray(value)) {
    return `[${value.length} items]`;
  }
  if (typeof value === "object" && value !== null) {
    const keys = Object.keys(value);
    if (keys.length <= 2) {
      return `{${keys.join(", ")}}`;
    }
    return `{${keys.length} keys}`;
  }
  return String(value);
}

function TaskNodeInner({
  task,
  isSelected,
  isStartNode,
  onSelect,
  onDelete,
  onPositionChange,
  onStartConnection,
  connectingFrom,
  onCompleteConnection,
  screenToCanvas,
}: TaskNodeProps) {
  const nodeRef = useRef<HTMLDivElement>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [hoveredHandle, setHoveredHandle] = useState<TransitionPreset | null>(
    null,
  );
  const [showTooltip, setShowTooltip] = useState(false);
  const tooltipTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);
  const dragOffset = useRef({ x: 0, y: 0 });

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      const target = e.target as HTMLElement;
      if (target.closest("[data-action-button]")) return;
      if (target.closest("[data-handle]")) return;

      e.stopPropagation();
      setIsDragging(true);
      setShowTooltip(false);

      // Convert the initial click to canvas-space so dragging stays
      // accurate regardless of the current zoom / pan.
      const canvasPos = screenToCanvas(e.clientX, e.clientY);
      dragOffset.current = {
        x: canvasPos.x - task.position.x,
        y: canvasPos.y - task.position.y,
      };

      const handleMouseMove = (moveEvent: MouseEvent) => {
        const cur = screenToCanvas(moveEvent.clientX, moveEvent.clientY);
        onPositionChange(task.id, {
          x: cur.x - dragOffset.current.x,
          y: cur.y - dragOffset.current.y,
        });
      };

      const handleMouseUp = () => {
        setIsDragging(false);
        document.removeEventListener("mousemove", handleMouseMove);
        document.removeEventListener("mouseup", handleMouseUp);
      };

      document.addEventListener("mousemove", handleMouseMove);
      document.addEventListener("mouseup", handleMouseUp);
    },
    [
      task.id,
      task.position.x,
      task.position.y,
      onPositionChange,
      screenToCanvas,
    ],
  );

  const handleClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      if (connectingFrom) {
        onCompleteConnection(task.id);
      } else {
        onSelect(task.id);
      }
    },
    [task.id, onSelect, connectingFrom, onCompleteConnection],
  );

  const handleDelete = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      onDelete(task.id);
    },
    [task.id, onDelete],
  );

  const handleHandleMouseDown = useCallback(
    (e: React.MouseEvent, preset: TransitionPreset) => {
      e.stopPropagation();
      e.preventDefault();
      onStartConnection(task.id, preset);
    },
    [task.id, onStartConnection],
  );

  const handleBodyMouseEnter = useCallback(() => {
    if (isDragging) return;
    tooltipTimeout.current = setTimeout(() => setShowTooltip(true), 400);
  }, [isDragging]);

  const handleBodyMouseLeave = useCallback(() => {
    if (tooltipTimeout.current) {
      clearTimeout(tooltipTimeout.current);
      tooltipTimeout.current = null;
    }
    setShowTooltip(false);
  }, []);

  const isConnectionTarget = connectingFrom !== null;

  const borderColor = isSelected
    ? "border-blue-500 ring-2 ring-blue-200"
    : isConnectionTarget
      ? "border-purple-400 ring-2 ring-purple-200"
      : "border-gray-300 hover:border-gray-400";

  const hasAction = task.action && task.action.length > 0;
  const summary = transitionSummary(task);

  // A stop node has no outgoing transitions to other tasks
  const isStopNode =
    !task.next ||
    task.next.length === 0 ||
    task.next.every((t) => !t.do || t.do.length === 0);

  // Count custom transitions (those not matching any preset)
  const customTransitionCount = (task.next || []).filter((t) => {
    const ct = classifyTransitionWhen(t.when);
    return ct === "custom";
  }).length;

  // Inputs that actually have values
  const populatedInputs = getPopulatedInputs(task.input);
  const populatedCount = populatedInputs.length;

  // Show inline if 1–2 populated inputs
  const showInlineInputs = populatedCount > 0 && populatedCount <= 2;

  // Build tooltip lines
  const tooltipLines: string[] = [];
  if (populatedCount > 0) {
    tooltipLines.push(
      `${populatedCount} input${populatedCount !== 1 ? "s" : ""} configured`,
    );
    // Show all input key-values in tooltip when > 2
    if (populatedCount > 2) {
      for (const [key, val] of populatedInputs) {
        tooltipLines.push(`  ${key}: ${formatValueTooltip(val)}`);
      }
    }
  }
  if (summary) {
    tooltipLines.push(summary);
  }
  if (customTransitionCount > 0) {
    tooltipLines.push(
      `${customTransitionCount} custom transition${customTransitionCount !== 1 ? "s" : ""}`,
    );
  }
  if (task.delay) {
    tooltipLines.push(`Delay: ${task.delay}s`);
  }
  if (task.with_items) {
    tooltipLines.push("with_items iteration");
  }
  if (task.iterate_cache) {
    tooltipLines.push(
      `Cache iteration: ${task.iterate_cache.namespace || "namespace required"}`,
    );
    tooltipLines.push(`Cache page size: ${task.iterate_cache.page_size}`);
    if (task.batch_size) {
      tooltipLines.push(
        `Cache batch size: ${task.batch_size} (item is ${task.batch_size === 1 ? "an object" : "an array"})`,
      );
    }
  }
  if (task.retry) {
    tooltipLines.push(`Retry: ${task.retry.count}×`);
  }
  if (task.timeout) {
    const timeoutLabel =
      typeof task.timeout === "string" ? task.timeout : `${task.timeout}s`;
    tooltipLines.push(`Timeout: ${timeoutLabel}`);
  }

  const hasTooltipContent = tooltipLines.length > 0;

  return (
    <div
      ref={nodeRef}
      className={`absolute select-none ${isDragging ? "cursor-grabbing z-50" : "cursor-grab z-10"}`}
      style={{
        left: task.position.x,
        top: task.position.y,
        width: 240,
      }}
      onMouseDown={handleMouseDown}
      onClick={handleClick}
      onMouseUp={(e) => {
        if (connectingFrom) {
          e.stopPropagation();
          onCompleteConnection(task.id);
        }
      }}
    >
      <div
        className={`bg-white rounded-lg border-2 shadow-sm transition-colors ${borderColor}`}
      >
        {/* Header */}
        <div
          className={`flex items-center gap-1.5 px-2.5 py-1.5 rounded-t-md border-b ${
            isStartNode
              ? "bg-green-500 bg-opacity-15 border-green-200"
              : "bg-blue-500 bg-opacity-10 border-gray-100"
          }`}
        >
          {isStartNode ? (
            <Play className="w-3.5 h-3.5 text-green-600 flex-shrink-0" />
          ) : (
            <GripVertical className="w-3.5 h-3.5 text-gray-400 flex-shrink-0" />
          )}
          <div className="flex-1 min-w-0">
            <div
              className={`font-semibold text-xs truncate ${
                isStartNode ? "text-green-900" : "text-gray-900"
              }`}
            >
              {task.name}
            </div>
          </div>
        </div>

        {/* Body */}
        <div
          className="px-2.5 py-2 relative"
          onMouseEnter={handleBodyMouseEnter}
          onMouseLeave={handleBodyMouseLeave}
        >
          {hasAction ? (
            <div className="flex items-center gap-1.5">
              <PackIcon
                packRef={packRefFromComponentRef(task.action)}
                size="xs"
              />
              <div className="font-mono text-[11px] text-gray-600 truncate">
                {task.action}
              </div>
            </div>
          ) : (
            <div className="text-[11px] text-orange-500 italic">
              No action assigned
            </div>
          )}

          {/* Inline inputs (1–2 populated) */}
          {showInlineInputs && (
            <div className="mt-1.5 space-y-0.5">
              {populatedInputs.map(([key, val]) => (
                <div
                  key={key}
                  className="flex items-baseline gap-1 text-[10px] leading-tight"
                >
                  <span className="text-gray-400 font-medium shrink-0">
                    {key}:
                  </span>
                  <span className="text-gray-600 truncate font-mono">
                    {formatValueShort(val)}
                  </span>
                </div>
              ))}
            </div>
          )}

          {/* Delay badge */}
          {task.delay && (
            <div className="mt-1 inline-block px-1.5 py-0.5 bg-yellow-50 border border-yellow-200 rounded text-[10px] text-yellow-700 truncate max-w-full">
              delay: {task.delay}s
            </div>
          )}

          {/* With-items badge */}
          {task.with_items && (
            <div className="mt-1 inline-block px-1.5 py-0.5 bg-indigo-50 border border-indigo-200 rounded text-[10px] text-indigo-700 truncate max-w-full">
              with_items
            </div>
          )}

          {/* Cache iteration badge */}
          {task.iterate_cache && (
            <div className="mt-1 inline-block px-1.5 py-0.5 bg-cyan-50 border border-cyan-200 rounded text-[10px] text-cyan-700 truncate max-w-full">
              iterate_cache
            </div>
          )}

          {/* Retry badge */}
          {task.retry && (
            <div className="mt-1 inline-block px-1.5 py-0.5 bg-orange-50 border border-orange-200 rounded text-[10px] text-orange-700 ml-1">
              retry: {task.retry.count}×
            </div>
          )}

          {/* Info icon hint — shown when there's tooltip content */}
          {hasTooltipContent && (
            <div className="absolute top-1.5 right-1.5">
              <Info className="w-3 h-3 text-gray-300" />
            </div>
          )}

          {/* Tooltip */}
          {showTooltip && hasTooltipContent && (
            <div
              className="absolute left-1/2 -translate-x-1/2 bottom-full mb-2 z-[100] pointer-events-none"
              style={{ minWidth: 180, maxWidth: 260 }}
            >
              <div className="bg-gray-900 text-white text-[10px] leading-relaxed rounded-md shadow-xl px-2.5 py-2 whitespace-pre-wrap">
                {tooltipLines.map((line, i) => (
                  <div
                    key={i}
                    className={
                      line.startsWith("  ")
                        ? "pl-2 text-gray-300 font-mono"
                        : i > 0
                          ? "mt-1 border-t border-gray-700 pt-1"
                          : ""
                    }
                  >
                    {line}
                  </div>
                ))}
              </div>
              <div className="absolute left-1/2 -translate-x-1/2 -bottom-1 w-2 h-2 bg-gray-900 rotate-45" />
            </div>
          )}
        </div>

        {/* Footer actions */}
        <div
          className={`flex items-center px-2 py-1.5 border-t rounded-b-md ${
            isStopNode
              ? "border-red-200 bg-red-50"
              : "border-gray-100 bg-gray-50"
          } ${isStopNode ? "justify-between" : "justify-end"}`}
        >
          {isStopNode && (
            <div className="flex items-center gap-1">
              <Octagon
                className="w-3.5 h-3.5 text-red-500"
                fill="currentColor"
                strokeWidth={0}
              />
              <span className="text-[10px] font-medium text-red-600">Stop</span>
            </div>
          )}
          <button
            data-action-button
            onClick={handleDelete}
            className="p-1 rounded hover:bg-red-100 text-gray-400 hover:text-red-600 transition-colors"
            title="Delete task"
          >
            <Trash2 className="w-3 h-3" />
          </button>
        </div>

        {/* Connection target overlay */}
        {isConnectionTarget && (
          <div className="absolute inset-0 rounded-lg bg-purple-100 bg-opacity-20 pointer-events-none flex items-center justify-center">
            <div className="text-xs font-medium text-purple-600 bg-white px-2 py-1 rounded shadow-sm">
              {connectingFrom?.taskId === task.id
                ? "Drop to self-loop"
                : "Drop to connect"}
            </div>
          </div>
        )}
      </div>

      {/* Output handles (bottom) — drag sources */}
      <div
        className="flex items-center justify-center gap-3 -mt-[7px] relative z-20"
        data-handle
      >
        {HANDLE_CONFIG.map((handle) => {
          const isActive = hasActiveTransition(task, handle.preset);
          const isHovered = hoveredHandle === handle.preset;
          const isCurrentlyDragging =
            connectingFrom?.taskId === task.id &&
            connectingFrom?.preset === handle.preset;

          return (
            <div
              key={handle.preset}
              className="relative group"
              onMouseEnter={() => setHoveredHandle(handle.preset)}
              onMouseLeave={() => setHoveredHandle(null)}
            >
              <div
                data-handle
                onMouseDown={(e) => handleHandleMouseDown(e, handle.preset)}
                className="transition-all duration-150 rounded-full border-2 border-white cursor-crosshair"
                style={{
                  width: isHovered || isCurrentlyDragging ? 14 : 10,
                  height: isHovered || isCurrentlyDragging ? 14 : 10,
                  backgroundColor: isCurrentlyDragging
                    ? handle.activeColor
                    : isHovered
                      ? handle.hoverColor
                      : isActive
                        ? handle.color
                        : `${handle.color}80`,
                  boxShadow: isCurrentlyDragging
                    ? `0 0 0 4px ${handle.ringColor}, 0 1px 3px rgba(0,0,0,0.2)`
                    : isHovered
                      ? `0 0 0 3px ${handle.ringColor}, 0 1px 2px rgba(0,0,0,0.15)`
                      : "0 1px 2px rgba(0,0,0,0.1)",
                }}
              />
              {/* Tooltip */}
              <div
                className={`absolute left-1/2 -translate-x-1/2 top-full mt-1.5 px-2 py-1 bg-gray-900 text-white text-[10px] font-medium rounded shadow-lg whitespace-nowrap pointer-events-none transition-opacity duration-150 ${
                  isHovered ? "opacity-100" : "opacity-0"
                }`}
              >
                {PRESET_LABELS[handle.preset]}
                <div className="absolute left-1/2 -translate-x-1/2 -top-1 w-2 h-2 bg-gray-900 rotate-45" />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

const TaskNode = memo(TaskNodeInner);
export default TaskNode;
