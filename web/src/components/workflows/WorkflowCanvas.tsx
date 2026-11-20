import { useState, useCallback, useRef, useMemo, useEffect } from "react";
import TaskNode from "./TaskNode";
import type { TransitionPreset } from "./TaskNode";
import WorkflowEdges from "./WorkflowEdges";
import type { EdgeHoverInfo, SelectedEdgeInfo } from "./WorkflowEdges";
import type {
  WorkflowTask,
  WorkflowEdge,
  NodePosition,
} from "@/types/workflow";
import {
  deriveEdges,
  generateUniqueTaskName,
  generateTaskId,
  findStartingTaskIds,
  PRESET_LABELS,
} from "@/types/workflow";
import { Plus, Maximize } from "lucide-react";
import { useInstallationSettings } from "@/contexts/InstallationSettingsContext";

interface WorkflowCanvasProps {
  tasks: WorkflowTask[];
  selectedTaskId: string | null;
  onSelectTask: (taskId: string | null) => void;
  onUpdateTask: (taskId: string, updates: Partial<WorkflowTask>) => void;
  onDeleteTask: (taskId: string) => void;
  onAddTask: (task: WorkflowTask) => void;
  onSetConnection: (
    fromTaskId: string,
    preset: TransitionPreset,
    toTaskName: string,
  ) => void;
  onEdgeClick?: (info: EdgeHoverInfo | null) => void;
}

/** Label color mapping for the connecting banner */
const PRESET_BANNER_COLORS: Record<TransitionPreset, string> = {
  succeeded: "text-green-200 font-bold",
  failed: "text-red-200 font-bold",
  always: "text-gray-200 font-bold",
};

const MIN_ZOOM = 0.15;
const MAX_ZOOM = 3;
const ZOOM_SENSITIVITY = 0.0015;
const CANVAS_SIDE_PADDING = 120;
const CANVAS_TOP_PADDING = 140;
const CANVAS_BOTTOM_PADDING = 120;
const CANVAS_RIGHT_PADDING = 380;

/**
 * Build CSS background style for the infinite grid.
 * Two layers: regular lines every 20 canvas-units, bold lines every 100 (5th).
 */
function gridBackground(pan: { x: number; y: number }, zoom: number) {
  const small = 20 * zoom;
  const large = 100 * zoom;
  return {
    backgroundImage: [
      `linear-gradient(to right, rgba(0,0,0,0.07) 1px, transparent 1px)`,
      `linear-gradient(to bottom, rgba(0,0,0,0.07) 1px, transparent 1px)`,
      `linear-gradient(to right, rgba(0,0,0,0.03) 1px, transparent 1px)`,
      `linear-gradient(to bottom, rgba(0,0,0,0.03) 1px, transparent 1px)`,
    ].join(","),
    backgroundSize: `${large}px ${large}px, ${large}px ${large}px, ${small}px ${small}px, ${small}px ${small}px`,
    backgroundPosition: `${pan.x}px ${pan.y}px, ${pan.x}px ${pan.y}px, ${pan.x}px ${pan.y}px, ${pan.x}px ${pan.y}px`,
  };
}

/**
 * Build a brick-lay tiled watermark using two CSS background layers.
 * Both layers repeat the same logo at the tile period, but the second
 * layer is offset by half the period in both axes for a staggered look.
 * Using background-size equal to the tile period causes the SVG to scale
 * to fill the tile — the logo's own viewBox whitespace provides the
 * visual padding around the mark.
 */
function watermarkBackground(pan: { x: number; y: number }, zoom: number) {
  const tileW = 1000 * zoom;
  const tileH = 700 * zoom;
  return {
    backgroundSize: `${tileW}px ${tileH}px, ${tileW}px ${tileH}px`,
    backgroundPosition: `${pan.x}px ${pan.y}px, ${pan.x + tileW / 2}px ${pan.y + tileH / 2}px`,
  };
}

export type ScreenToCanvas = (
  clientX: number,
  clientY: number,
) => { x: number; y: number };

export default function WorkflowCanvas({
  tasks,
  selectedTaskId,
  onSelectTask,
  onUpdateTask,
  onDeleteTask,
  onAddTask,
  onSetConnection,
  onEdgeClick,
}: WorkflowCanvasProps) {
  const { settings } = useInstallationSettings();
  const branding = settings?.branding;
  const watermarkUrl =
    branding?.workflow_watermark_url ||
    (branding?.logo_url || branding?.icon_url
      ? null
      : "/attune-logo-watermark-tile.svg");
  const watermarkImage = watermarkUrl
    ? `url(${JSON.stringify(watermarkUrl)})`
    : "none";
  const canvasRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const watermarkRef = useRef<HTMLDivElement>(null);

  // ---- Camera state ----
  // We keep refs for high-frequency updates (panning/zooming) and sync to
  // state on mouseup / wheel-end so React can re-render once.
  const panRef = useRef({ x: 0, y: 0 });
  const zoomRef = useRef(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [zoom, setZoom] = useState(1);

  // ---- Connection state ----
  const [connectingFrom, setConnectingFrom] = useState<{
    taskId: string;
    preset: TransitionPreset;
  } | null>(null);
  const [mousePosition, setMousePosition] = useState<{
    x: number;
    y: number;
  } | null>(null);

  // ---- Panning state (right-click drag) ----
  const isPanning = useRef(false);
  const panDragStart = useRef({ x: 0, y: 0, panX: 0, panY: 0 });
  const [panningCursor, setPanningCursor] = useState(false);

  const [selectedEdge, setSelectedEdge] = useState<SelectedEdgeInfo | null>(
    null,
  );

  const allTaskNames = useMemo(() => tasks.map((t) => t.name), [tasks]);
  const edges: WorkflowEdge[] = useMemo(() => deriveEdges(tasks), [tasks]);
  const startingTaskIds = useMemo(() => findStartingTaskIds(tasks), [tasks]);

  // ---- Coordinate conversion ----
  /** Convert screen (client) coordinates to canvas-space coordinates. */
  const screenToCanvas: ScreenToCanvas = useCallback(
    (clientX: number, clientY: number) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return { x: clientX, y: clientY };
      return {
        x: (clientX - rect.left - panRef.current.x) / zoomRef.current,
        y: (clientY - rect.top - panRef.current.y) / zoomRef.current,
      };
    },
    [],
  );

  // ---- Flush camera refs to React state (triggers re-render) ----
  const commitCamera = useCallback(() => {
    setPan({ ...panRef.current });
    setZoom(zoomRef.current);
  }, []);

  /** Apply current ref values directly to the DOM for smooth animation. */
  const applyTransformToDOM = useCallback(() => {
    if (innerRef.current) {
      innerRef.current.style.transform = `translate(${panRef.current.x}px, ${panRef.current.y}px) scale(${zoomRef.current})`;
    }
    if (canvasRef.current) {
      const bg = gridBackground(panRef.current, zoomRef.current);
      canvasRef.current.style.backgroundSize = bg.backgroundSize;
      canvasRef.current.style.backgroundPosition = bg.backgroundPosition;
    }
    if (watermarkRef.current) {
      const wm = watermarkBackground(panRef.current, zoomRef.current);
      watermarkRef.current.style.backgroundSize = wm.backgroundSize;
      watermarkRef.current.style.backgroundPosition = wm.backgroundPosition;
    }
  }, []);

  // ---- Canvas click (deselect / cancel connection) ----
  const handleCanvasClick = useCallback(
    (e: React.MouseEvent) => {
      const target = e.target as HTMLElement;
      if (
        target === canvasRef.current ||
        target === innerRef.current ||
        target.dataset.canvasBg === "true"
      ) {
        if (connectingFrom) {
          setConnectingFrom(null);
          setMousePosition(null);
        } else {
          onSelectTask(null);
          setSelectedEdge(null);
          onEdgeClick?.(null);
        }
      }
    },
    [onSelectTask, onEdgeClick, connectingFrom],
  );

  // ---- Mouse move: panning + connection preview ----
  const handleCanvasMouseMove = useCallback(
    (e: React.MouseEvent) => {
      // Right-click panning (direct DOM, no React re-render)
      if (isPanning.current) {
        const dx = e.clientX - panDragStart.current.x;
        const dy = e.clientY - panDragStart.current.y;
        panRef.current = {
          x: panDragStart.current.panX + dx,
          y: panDragStart.current.panY + dy,
        };
        applyTransformToDOM();
        return;
      }

      // Connection preview line
      if (connectingFrom) {
        const pos = screenToCanvas(e.clientX, e.clientY);
        setMousePosition(pos);
      }
    },
    [connectingFrom, screenToCanvas, applyTransformToDOM],
  );

  // ---- Mouse down: start panning on right-click ----
  const handleCanvasMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button === 2) {
      e.preventDefault();
      isPanning.current = true;
      panDragStart.current = {
        x: e.clientX,
        y: e.clientY,
        panX: panRef.current.x,
        panY: panRef.current.y,
      };
      setPanningCursor(true);
    }
  }, []);

  // ---- Mouse up: stop panning / cancel connection ----
  const handleCanvasMouseUp = useCallback(
    (e: React.MouseEvent) => {
      if (e.button === 2 && isPanning.current) {
        isPanning.current = false;
        setPanningCursor(false);
        commitCamera();
        return;
      }
      if (connectingFrom) {
        setConnectingFrom(null);
        setMousePosition(null);
      }
    },
    [connectingFrom, commitCamera],
  );

  // ---- Context menu suppression ----
  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
  }, []);

  // ---- Scroll wheel: zoom centred on cursor ----
  // Must be a non-passive imperative listener so preventDefault() reliably
  // stops the page from scrolling. React's onWheel is passive in some browsers.
  const handleWheel = useCallback(
    (e: WheelEvent) => {
      e.preventDefault();
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return;

      const mouseScreenX = e.clientX - rect.left;
      const mouseScreenY = e.clientY - rect.top;

      const oldZoom = zoomRef.current;
      const delta = -e.deltaY * ZOOM_SENSITIVITY;
      const newZoom = Math.min(
        MAX_ZOOM,
        Math.max(MIN_ZOOM, oldZoom * (1 + delta)),
      );

      // Adjust pan so the point under the cursor stays fixed
      const scale = newZoom / oldZoom;
      panRef.current = {
        x: mouseScreenX - (mouseScreenX - panRef.current.x) * scale,
        y: mouseScreenY - (mouseScreenY - panRef.current.y) * scale,
      };
      zoomRef.current = newZoom;

      applyTransformToDOM();
      commitCamera();
    },
    [applyTransformToDOM, commitCamera],
  );

  // Attach wheel listener imperatively with { passive: false }
  useEffect(() => {
    const el = canvasRef.current;
    if (!el) return;
    el.addEventListener("wheel", handleWheel, { passive: false });
    return () => el.removeEventListener("wheel", handleWheel);
  }, [handleWheel]);

  // Safety: cancel panning if mouse leaves the window
  useEffect(() => {
    const handleGlobalMouseUp = () => {
      if (isPanning.current) {
        isPanning.current = false;
        setPanningCursor(false);
        commitCamera();
      }
    };
    window.addEventListener("mouseup", handleGlobalMouseUp);
    return () => window.removeEventListener("mouseup", handleGlobalMouseUp);
  }, [commitCamera]);

  // ---- Node interactions ----
  const handlePositionChange = useCallback(
    (taskId: string, position: { x: number; y: number }) => {
      onUpdateTask(taskId, { position });
    },
    [onUpdateTask],
  );

  const handleStartConnection = useCallback(
    (taskId: string, preset: TransitionPreset) => {
      setConnectingFrom({ taskId, preset });
      setSelectedEdge(null);
      onEdgeClick?.(null);
    },
    [onEdgeClick],
  );

  const handleEdgeClick = useCallback(
    (info: EdgeHoverInfo | null) => {
      if (info) {
        setSelectedEdge({
          from: info.taskId,
          to: info.targetTaskId,
          transitionIndex: info.transitionIndex,
        });
      } else {
        setSelectedEdge(null);
      }
      onEdgeClick?.(info);
    },
    [onEdgeClick],
  );

  const handleSelectTask = useCallback(
    (taskId: string | null) => {
      onSelectTask(taskId);
      if (taskId !== null) {
        if (selectedEdge && selectedEdge.from !== taskId) {
          setSelectedEdge(null);
          onEdgeClick?.(null);
        }
      }
    },
    [onSelectTask, onEdgeClick, selectedEdge],
  );

  const handleWaypointUpdate = useCallback(
    (
      fromTaskId: string,
      transitionIndex: number,
      targetTaskName: string,
      waypoints: NodePosition[],
    ) => {
      const task = tasks.find((t) => t.id === fromTaskId);
      if (!task || !task.next || transitionIndex >= task.next.length) return;

      const updatedNext = [...task.next];
      const transition = { ...updatedNext[transitionIndex] };
      const edgeWaypoints = { ...(transition.edge_waypoints || {}) };

      if (waypoints.length > 0) {
        edgeWaypoints[targetTaskName] = waypoints;
      } else {
        delete edgeWaypoints[targetTaskName];
      }

      transition.edge_waypoints =
        Object.keys(edgeWaypoints).length > 0 ? edgeWaypoints : undefined;
      updatedNext[transitionIndex] = transition;
      onUpdateTask(fromTaskId, { next: updatedNext });
    },
    [tasks, onUpdateTask],
  );

  const handleLabelPositionUpdate = useCallback(
    (
      fromTaskId: string,
      transitionIndex: number,
      targetTaskName: string,
      position: number | undefined,
    ) => {
      const task = tasks.find((t) => t.id === fromTaskId);
      if (!task || !task.next || transitionIndex >= task.next.length) return;

      const updatedNext = [...task.next];
      const transition = { ...updatedNext[transitionIndex] };
      const labelPositions = { ...(transition.label_positions || {}) };

      if (position) {
        labelPositions[targetTaskName] = position;
      } else {
        delete labelPositions[targetTaskName];
      }

      transition.label_positions =
        Object.keys(labelPositions).length > 0 ? labelPositions : undefined;
      updatedNext[transitionIndex] = transition;
      onUpdateTask(fromTaskId, { next: updatedNext });
    },
    [tasks, onUpdateTask],
  );

  const handleCompleteConnection = useCallback(
    (targetTaskId: string) => {
      if (!connectingFrom) return;
      const targetTask = tasks.find((t) => t.id === targetTaskId);
      if (!targetTask) return;

      onSetConnection(
        connectingFrom.taskId,
        connectingFrom.preset,
        targetTask.name,
      );
      setConnectingFrom(null);
      setMousePosition(null);
    },
    [connectingFrom, tasks, onSetConnection],
  );

  const handleAddEmptyTask = useCallback(() => {
    const name = generateUniqueTaskName(tasks);
    let maxY = 0;
    for (const task of tasks) {
      if (task.position.y > maxY) maxY = task.position.y;
    }
    const newTask: WorkflowTask = {
      id: generateTaskId(),
      name,
      action: "",
      input: {},
      position: {
        x: 300,
        y: tasks.length === 0 ? 60 : maxY + 160,
      },
    };
    onAddTask(newTask);
    onSelectTask(newTask.id);
  }, [tasks, onAddTask, onSelectTask]);

  /** Reset pan/zoom to fit all tasks (or default viewport). */
  const handleFitView = useCallback(() => {
    if (tasks.length === 0) {
      panRef.current = { x: 0, y: 0 };
      zoomRef.current = 1;
    } else {
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect) return;

      let minX = Infinity,
        minY = Infinity,
        maxX = -Infinity,
        maxY = -Infinity;
      for (const t of tasks) {
        minX = Math.min(minX, t.position.x);
        minY = Math.min(minY, t.position.y);
        maxX = Math.max(maxX, t.position.x + 240);
        maxY = Math.max(maxY, t.position.y + 140);
      }

      minX -= CANVAS_SIDE_PADDING;
      minY -= CANVAS_TOP_PADDING;
      maxX += CANVAS_RIGHT_PADDING;
      maxY += CANVAS_BOTTOM_PADDING;

      const contentW = maxX - minX;
      const contentH = maxY - minY;
      const pad = 80;
      const scaleX = (rect.width - pad * 2) / contentW;
      const scaleY = (rect.height - pad * 2) / contentH;
      const newZoom = Math.min(
        Math.max(Math.min(scaleX, scaleY), MIN_ZOOM),
        MAX_ZOOM,
      );

      panRef.current = {
        x: (rect.width - contentW * newZoom) / 2 - minX * newZoom,
        y: (rect.height - contentH * newZoom) / 2 - minY * newZoom,
      };
      zoomRef.current = newZoom;
    }
    applyTransformToDOM();
    commitCamera();
  }, [tasks, applyTransformToDOM, commitCamera]);

  // ---- Inner div dimensions (large enough to contain all content) ----
  const innerSize = useMemo(() => {
    let minX = 0;
    let minY = 0;
    let maxX = 4000;
    let maxY = 4000;
    for (const task of tasks) {
      minX = Math.min(minX, task.position.x - CANVAS_SIDE_PADDING);
      minY = Math.min(minY, task.position.y - CANVAS_TOP_PADDING);
      maxX = Math.max(maxX, task.position.x + CANVAS_RIGHT_PADDING);
      maxY = Math.max(maxY, task.position.y + CANVAS_BOTTOM_PADDING + 380);
    }
    return { width: maxX - minX, height: maxY - minY };
  }, [tasks]);

  // ---- Grid background (recomputed from React state for the render) ----
  const gridBg = useMemo(() => gridBackground(pan, zoom), [pan, zoom]);
  const wmBg = useMemo(() => watermarkBackground(pan, zoom), [pan, zoom]);

  // Zoom percentage for display
  const zoomPercent = Math.round(zoom * 100);

  return (
    <div
      ref={canvasRef}
      className={`flex-1 overflow-hidden bg-gray-100 relative ${panningCursor ? "!cursor-grabbing" : ""}`}
      style={{
        backgroundImage: gridBg.backgroundImage,
        backgroundSize: gridBg.backgroundSize,
        backgroundPosition: gridBg.backgroundPosition,
      }}
      onClick={handleCanvasClick}
      onMouseDown={handleCanvasMouseDown}
      onMouseMove={handleCanvasMouseMove}
      onMouseUp={handleCanvasMouseUp}
      onContextMenu={handleContextMenu}
    >
      {/* Tiled watermark layer — moves with grid, transparent */}
      <div
        ref={watermarkRef}
        className="absolute inset-0 pointer-events-none opacity-[0.15]"
        style={{
          backgroundImage: `${watermarkImage}, ${watermarkImage}`,
          backgroundSize: wmBg.backgroundSize,
          backgroundPosition: wmBg.backgroundPosition,
        }}
      />

      {/* Transformed canvas content */}
      <div
        ref={innerRef}
        data-canvas-bg="true"
        style={{
          position: "absolute",
          transformOrigin: "0 0",
          transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`,
          width: innerSize.width,
          height: innerSize.height,
        }}
      >
        {/* Edge rendering layer */}
        <WorkflowEdges
          edges={edges}
          tasks={tasks}
          connectingFrom={connectingFrom}
          mousePosition={mousePosition}
          onEdgeClick={handleEdgeClick}
          selectedEdge={selectedEdge}
          onWaypointUpdate={handleWaypointUpdate}
          onLabelPositionUpdate={handleLabelPositionUpdate}
          screenToCanvas={screenToCanvas}
        />

        {/* Task nodes */}
        {tasks.map((task) => (
          <TaskNode
            key={task.id}
            task={task}
            isSelected={task.id === selectedTaskId}
            isStartNode={startingTaskIds.has(task.id)}
            allTaskNames={allTaskNames}
            onSelect={handleSelectTask}
            onDelete={onDeleteTask}
            onPositionChange={handlePositionChange}
            onStartConnection={handleStartConnection}
            connectingFrom={connectingFrom}
            onCompleteConnection={handleCompleteConnection}
            screenToCanvas={screenToCanvas}
          />
        ))}
      </div>

      {/* ---- UI chrome (not transformed) ---- */}

      {/* Connecting mode indicator */}
      {connectingFrom && (
        <div className="absolute top-0 left-0 right-0 z-50 flex justify-center pointer-events-none">
          <div className="mt-3 px-4 py-2 bg-purple-600 text-white text-sm font-medium rounded-full shadow-lg pointer-events-auto">
            Drag to a task to connect as{" "}
            <span className={PRESET_BANNER_COLORS[connectingFrom.preset]}>
              {PRESET_LABELS[connectingFrom.preset]}
            </span>{" "}
            transition — or release to cancel
          </div>
        </div>
      )}

      {/* Zoom indicator + fit-view button */}
      <div className="absolute bottom-6 left-6 z-40 flex items-center gap-2">
        <div className="px-2.5 py-1.5 bg-white/80 backdrop-blur-sm text-xs font-medium text-gray-500 rounded-lg shadow-sm border border-gray-200 select-none tabular-nums">
          {zoomPercent}%
        </div>
        <button
          onClick={handleFitView}
          className="p-1.5 bg-white/80 backdrop-blur-sm text-gray-500 rounded-lg shadow-sm border border-gray-200 hover:bg-white hover:text-gray-700 transition-colors"
          title="Fit view to content"
        >
          <Maximize className="w-3.5 h-3.5" />
        </button>
      </div>

      {/* Empty state / Add task button */}
      {tasks.length === 0 ? (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <div className="text-center pointer-events-auto">
            <div className="w-16 h-16 mx-auto mb-4 rounded-full bg-gray-200 flex items-center justify-center">
              <Plus className="w-8 h-8 text-gray-400" />
            </div>
            <h3 className="text-lg font-medium text-gray-600 mb-2">
              Empty Workflow
            </h3>
            <p className="text-sm text-gray-400 mb-4 max-w-xs">
              Add tasks from the action palette on the left, or click the button
              below to add a blank task.
            </p>
            <button
              onClick={handleAddEmptyTask}
              className="px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-lg hover:bg-blue-700 transition-colors shadow-sm"
            >
              <Plus className="w-4 h-4 inline-block mr-1.5 -mt-0.5" />
              Add First Task
            </button>
          </div>
        </div>
      ) : (
        <button
          onClick={handleAddEmptyTask}
          className="absolute bottom-6 right-6 z-40 w-12 h-12 bg-blue-600 text-white rounded-full shadow-lg hover:bg-blue-700 transition-colors flex items-center justify-center"
          title="Add a new task"
        >
          <Plus className="w-6 h-6" />
        </button>
      )}
    </div>
  );
}
