import { useState, useCallback, useMemo, useEffect, useRef } from "react";
import {
  X,
  Plus,
  Trash2,
  ChevronDown,
  ChevronRight,
  GripVertical,
  ArrowRight,
  Palette,
  Loader2,
} from "lucide-react";
import SearchableSelect from "@/components/common/SearchableSelect";
import type {
  WorkflowTask,
  RetryConfig,
  PaletteAction,
  TaskTransition,
  PublishDirective,
  LineStyle,
} from "@/types/workflow";
import {
  PRESET_WHEN,
  PRESET_LABELS,
  PRESET_COLORS,
  EDGE_TYPE_COLORS,
  classifyTransitionWhen,
  transitionLabel,
} from "@/types/workflow";
import ParamSchemaForm, {
  extractProperties,
  type ParamSchema,
} from "@/components/common/ParamSchemaForm";
import { useAction } from "@/hooks/useActions";

/** Preset color swatches for quick transition color selection */
const TRANSITION_COLOR_SWATCHES = [
  { color: "#22c55e", label: "Green" },
  { color: "#ef4444", label: "Red" },
  { color: "#6b7280", label: "Gray" },
  { color: "#8b5cf6", label: "Violet" },
  { color: "#3b82f6", label: "Blue" },
  { color: "#f59e0b", label: "Amber" },
  { color: "#ec4899", label: "Pink" },
  { color: "#14b8a6", label: "Teal" },
  { color: "#f97316", label: "Orange" },
];
import type { TransitionPreset } from "./TaskNode";

interface TaskInspectorProps {
  task: WorkflowTask;
  allTaskNames: string[];
  availableActions: PaletteAction[];
  onUpdate: (taskId: string, updates: Partial<WorkflowTask>) => void;
  onClose: () => void;
  /** When set, auto-expand transitions section, scroll to this index, and flash-highlight it */
  highlightTransitionIndex?: number | null;
}

/** Color dot for transition type */
const TRANSITION_DOT_COLORS: Record<string, string> = {
  success: "bg-green-500",
  failure: "bg-red-500",
  complete: "bg-gray-500",
  custom: "bg-violet-500",
};

function permissionSetRefsToInput(
  value: WorkflowTask["permission_set_refs"],
): string {
  if (Array.isArray(value)) {
    return value.join(", ");
  }
  return value ?? "";
}

function inputToPermissionSetRefs(
  value: string,
): WorkflowTask["permission_set_refs"] {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }

  if (trimmed.startsWith("{{") && trimmed.endsWith("}}")) {
    return trimmed;
  }

  if (trimmed.startsWith("[")) {
    try {
      const parsed = JSON.parse(trimmed);
      if (
        Array.isArray(parsed) &&
        parsed.every((item) => typeof item === "string")
      ) {
        return parsed.map((item) => item.trim()).filter(Boolean);
      }
    } catch {
      // Fall through to comma/newline parsing for partially typed values.
    }
  }

  return trimmed
    .split(/[,\n]/)
    .map((item) => item.trim())
    .filter(Boolean);
}

export default function TaskInspector({
  task,
  allTaskNames,
  availableActions,
  onUpdate,
  onClose,
  highlightTransitionIndex,
}: TaskInspectorProps) {
  // Fetch full action details (including param_schema) on demand
  const { data: actionDetail, isLoading: actionLoading } = useAction(
    task.action || "",
  );
  const transitionRefs = useRef<Map<number, HTMLDivElement>>(new Map());
  const [flashIndex, setFlashIndex] = useState<number | null>(null);
  const [expandedSections, setExpandedSections] = useState<Set<string>>(
    new Set(["basic", "action", "transitions"]),
  );

  // Use task.id as key to reset local state when switching tasks.
  const [trackedTaskId, setTrackedTaskId] = useState(task.id);
  const initialName = task.name;
  const initialWithItems = task.with_items || "";
  const initialTimeout = task.timeout ? String(task.timeout) : "";
  const initialDelay = task.delay ? String(task.delay) : "";

  if (trackedTaskId !== task.id) {
    setTrackedTaskId(task.id);
  }

  const [localName, setLocalName] = useState(initialName);
  const [localWithItems, setLocalWithItems] = useState(initialWithItems);
  const [localTimeout, setLocalTimeout] = useState(initialTimeout);
  const [localDelay, setLocalDelay] = useState(initialDelay);

  // Reset local state when the selected task changes
  if (trackedTaskId !== task.id) {
    setLocalName(initialName);
    setLocalWithItems(initialWithItems);
    setLocalTimeout(initialTimeout);
    setLocalDelay(initialDelay);
  }

  // Adjust state synchronously when highlightTransitionIndex prop changes
  // (React-approved pattern: https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes)
  const [prevHighlight, setPrevHighlight] = useState<number | null>(null);
  const effectiveHighlight = highlightTransitionIndex ?? null;
  if (effectiveHighlight !== prevHighlight) {
    setPrevHighlight(effectiveHighlight);
    if (effectiveHighlight != null) {
      // Ensure transitions section is expanded
      setExpandedSections((prev) => {
        if (prev.has("transitions")) return prev;
        const next = new Set(prev);
        next.add("transitions");
        return next;
      });
      // Trigger flash animation
      setFlashIndex(effectiveHighlight);
    }
  }

  // Scroll to the highlighted transition and auto-clear flash (side-effects only)
  useEffect(() => {
    if (flashIndex == null) return;

    // Wait a tick for the section to expand and refs to attach, then scroll
    const raf = requestAnimationFrame(() => {
      const el = transitionRefs.current.get(flashIndex);
      if (el) {
        el.scrollIntoView({ behavior: "smooth", block: "center" });
      }
    });

    // Clear flash after the animation completes
    const timeout = setTimeout(() => {
      setFlashIndex(null);
    }, 1500);

    return () => {
      cancelAnimationFrame(raf);
      clearTimeout(timeout);
    };
  }, [flashIndex]);

  const toggleSection = (section: string) => {
    setExpandedSections((prev) => {
      const next = new Set(prev);
      if (next.has(section)) {
        next.delete(section);
      } else {
        next.add(section);
      }
      return next;
    });
  };

  const update = useCallback(
    (updates: Partial<WorkflowTask>) => {
      onUpdate(task.id, updates);
    },
    [task.id, onUpdate],
  );

  const updateIterateCache = useCallback(
    (updates: Partial<NonNullable<WorkflowTask["iterate_cache"]>>) => {
      if (!task.iterate_cache) return;
      update({ iterate_cache: { ...task.iterate_cache, ...updates } });
    },
    [task.iterate_cache, update],
  );

  // --- Transition helpers ---

  const transitions = task.next || [];

  const updateTransition = useCallback(
    (index: number, updates: Partial<TaskTransition>) => {
      const next = [...(task.next || [])];
      next[index] = { ...next[index], ...updates };
      update({ next });
    },
    [task.next, update],
  );

  const removeTransition = useCallback(
    (index: number) => {
      const next = [...(task.next || [])];
      next.splice(index, 1);
      update({ next: next.length > 0 ? next : undefined });
    },
    [task.next, update],
  );

  const addTransition = useCallback(
    (preset?: TransitionPreset) => {
      const next = [...(task.next || [])];
      const newTransition: TaskTransition = {};
      if (preset) {
        const whenExpr = PRESET_WHEN[preset];
        if (whenExpr) newTransition.when = whenExpr;
        newTransition.label = PRESET_LABELS[preset];
        newTransition.color = PRESET_COLORS[preset];
        if (preset === "failed") {
          newTransition.line_style = "dashed";
        }
      }
      next.push(newTransition);
      update({ next });
    },
    [task.next, update],
  );

  const addDoTarget = useCallback(
    (transitionIndex: number, targetName: string) => {
      const next = [...(task.next || [])];
      const transition = { ...next[transitionIndex] };
      const doList = [...(transition.do || [])];
      if (!doList.includes(targetName)) {
        doList.push(targetName);
      }
      transition.do = doList;
      next[transitionIndex] = transition;
      update({ next });
    },
    [task.next, update],
  );

  const removeDoTarget = useCallback(
    (transitionIndex: number, targetIndex: number) => {
      const next = [...(task.next || [])];
      const transition = { ...next[transitionIndex] };
      const doList = [...(transition.do || [])];
      doList.splice(targetIndex, 1);
      transition.do = doList.length > 0 ? doList : undefined;
      next[transitionIndex] = transition;
      update({ next });
    },
    [task.next, update],
  );

  const addPublishDirective = useCallback(
    (transitionIndex: number) => {
      const next = [...(task.next || [])];
      const transition = { ...next[transitionIndex] };
      transition.publish = [...(transition.publish || []), { "": "" }];
      next[transitionIndex] = transition;
      update({ next });
    },
    [task.next, update],
  );

  const updatePublishDirective = useCallback(
    (
      transitionIndex: number,
      publishIndex: number,
      directive: PublishDirective,
    ) => {
      const next = [...(task.next || [])];
      const transition = { ...next[transitionIndex] };
      const publish = [...(transition.publish || [])];
      publish[publishIndex] = directive;
      transition.publish = publish;
      next[transitionIndex] = transition;
      update({ next });
    },
    [task.next, update],
  );

  const removePublishDirective = useCallback(
    (transitionIndex: number, publishIndex: number) => {
      const next = [...(task.next || [])];
      const transition = { ...next[transitionIndex] };
      const publish = [...(transition.publish || [])];
      publish.splice(publishIndex, 1);
      transition.publish = publish.length > 0 ? publish : undefined;
      next[transitionIndex] = transition;
      update({ next });
    },
    [task.next, update],
  );

  // Get the selected action's param schema from fetched action detail
  const selectedAction = availableActions.find((a) => a.ref === task.action);
  const fetchedAction = actionDetail?.data;
  const actionParamSchema: ParamSchema = useMemo(
    () => (fetchedAction?.param_schema as ParamSchema) || {},
    [fetchedAction?.param_schema],
  );
  const schemaProperties = useMemo(
    () => extractProperties(actionParamSchema),
    [actionParamSchema],
  );
  const hasSchema = Object.keys(schemaProperties).length > 0;

  const otherTaskNames = allTaskNames.filter((n) => n !== task.name);

  return (
    <div className="w-80 border-l border-gray-200 bg-white flex flex-col h-full overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-gray-200 bg-gray-50 flex-shrink-0">
        <div className="min-w-0">
          <h3 className="text-sm font-semibold text-gray-900 truncate">
            Configure Task
          </h3>
          <p className="text-xs text-gray-500 truncate mt-0.5">{task.name}</p>
        </div>
        <button
          onClick={onClose}
          className="p-1 rounded hover:bg-gray-200 text-gray-400 hover:text-gray-600 transition-colors flex-shrink-0"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      {/* Scrollable content */}
      <div className="flex-1 overflow-y-auto">
        {/* Basic Section */}
        <CollapsibleSection
          title="Basic"
          sectionKey="basic"
          expanded={expandedSections.has("basic")}
          onToggle={toggleSection}
        >
          <div className="space-y-3">
            {/* Task Name */}
            <div>
              <label className="block text-xs font-medium text-gray-700 mb-1">
                Task Name
              </label>
              <input
                type="text"
                value={localName}
                onChange={(e) => setLocalName(e.target.value)}
                onBlur={() => {
                  if (localName.trim() && localName !== task.name) {
                    update({ name: localName.trim() });
                  }
                }}
                className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                placeholder="task_name"
              />
            </div>
          </div>
        </CollapsibleSection>

        {/* Action Section */}
        <CollapsibleSection
          title="Action"
          sectionKey="action"
          expanded={expandedSections.has("action")}
          onToggle={toggleSection}
        >
          <div className="space-y-3">
            {/* Action Reference */}
            <div>
              <label className="block text-xs font-medium text-gray-700 mb-1">
                Action Reference
              </label>
              <SearchableSelect
                value={task.action || ""}
                onChange={(v) => update({ action: String(v) })}
                options={availableActions.map((action) => ({
                  value: action.ref,
                  label: `${action.ref} (${action.label})`,
                }))}
                placeholder="-- Select an action --"
              />
              {(fetchedAction?.description || selectedAction?.description) && (
                <p className="text-[10px] text-gray-400 mt-1">
                  {fetchedAction?.description || selectedAction?.description}
                </p>
              )}
            </div>

            {/* Input Parameters — schema-driven form */}
            {task.action && actionLoading && (
              <div className="flex items-center gap-2 text-xs text-gray-400 py-2">
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                Loading parameters…
              </div>
            )}
            {hasSchema && (
              <div>
                <label className="block text-xs font-medium text-gray-700 mb-1.5">
                  Input Parameters
                </label>
                <ParamSchemaForm
                  schema={actionParamSchema}
                  values={task.input}
                  onChange={(newValues) => {
                    update({ input: newValues });
                  }}
                  allowTemplates
                  hideTemplateHint
                  className="text-xs"
                />
              </div>
            )}
            {task.action && !actionLoading && !hasSchema && (
              <p className="text-[10px] text-gray-400 italic">
                This action has no declared parameters.
              </p>
            )}

            <div>
              <label className="block text-xs font-medium text-gray-700 mb-1">
                Execution Permission Set Refs
              </label>
              <textarea
                value={permissionSetRefsToInput(task.permission_set_refs)}
                onChange={(e) =>
                  update({
                    permission_set_refs: inputToPermissionSetRefs(
                      e.target.value,
                    ),
                  })
                }
                className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-xs font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                placeholder="core.executor_mcp, my_pack.agent_scope or {{ workflow.permission_sets }}"
                rows={2}
              />
              <p className="text-[10px] text-gray-400 mt-0.5">
                Optional. Use comma/newline-separated refs, a JSON string array,
                or a template resolving to a ref or ref array. When omitted, the
                action&apos;s default execution permission sets are used.
              </p>
            </div>
          </div>
        </CollapsibleSection>

        {/* Transitions Section (Orquesta-style next array) */}
        <CollapsibleSection
          title={`Transitions (${transitions.length})`}
          sectionKey="transitions"
          expanded={expandedSections.has("transitions")}
          onToggle={toggleSection}
        >
          <div className="space-y-3">
            <p className="text-[10px] text-gray-400">
              Transitions are evaluated in order after this task completes. Each
              can have a condition, publish variables, and target tasks.
            </p>

            {/* Transition list */}
            {transitions.map((transition, ti) => {
              const edgeType = classifyTransitionWhen(transition.when);
              const label = transitionLabel(transition.when, transition.label);
              const isFlashing = flashIndex === ti;

              return (
                <div
                  key={ti}
                  ref={(el) => {
                    if (el) {
                      transitionRefs.current.set(ti, el);
                    } else {
                      transitionRefs.current.delete(ti);
                    }
                  }}
                  className={`border rounded-lg bg-gray-50 overflow-hidden transition-all duration-300 ${
                    isFlashing
                      ? "border-blue-400 ring-2 ring-blue-300 shadow-md shadow-blue-100 animate-[flash-highlight_1.5s_ease-out]"
                      : highlightTransitionIndex === ti
                        ? "border-blue-400 ring-1 ring-blue-200 bg-blue-50/40"
                        : "border-gray-200"
                  }`}
                >
                  {/* Transition header */}
                  <div className="flex items-center gap-2 px-2.5 py-2 bg-white border-b border-gray-100">
                    <GripVertical className="w-3 h-3 text-gray-300 flex-shrink-0" />
                    {transition.color ? (
                      <div
                        className="w-2.5 h-2.5 rounded-full flex-shrink-0"
                        style={{ backgroundColor: transition.color }}
                      />
                    ) : (
                      <div
                        className={`w-2.5 h-2.5 rounded-full flex-shrink-0 ${TRANSITION_DOT_COLORS[edgeType]}`}
                      />
                    )}
                    <span className="text-[11px] font-medium text-gray-700 flex-1 truncate">
                      {label}
                    </span>
                    <button
                      onClick={() => removeTransition(ti)}
                      className="p-0.5 text-gray-400 hover:text-red-500 flex-shrink-0"
                      title="Remove transition"
                    >
                      <Trash2 className="w-3 h-3" />
                    </button>
                  </div>

                  <div className="px-2.5 py-2 space-y-2.5">
                    {/* Transition label (rename) */}
                    <div>
                      <label className="block text-[10px] font-medium text-gray-500 mb-0.5">
                        Label
                      </label>
                      <input
                        type="text"
                        value={transition.label || ""}
                        onChange={(e) =>
                          updateTransition(ti, {
                            label: e.target.value || undefined,
                          })
                        }
                        className="w-full px-2 py-1 border border-gray-300 rounded text-xs focus:ring-1 focus:ring-blue-500 focus:border-blue-500"
                        placeholder={transitionLabel(transition.when)}
                      />
                    </div>

                    {/* Transition color */}
                    <div>
                      <label className="block text-[10px] font-medium text-gray-500 mb-0.5">
                        <Palette className="w-3 h-3 inline-block mr-0.5 -mt-0.5" />
                        Color
                      </label>
                      <div className="flex items-center gap-1.5 flex-wrap">
                        {TRANSITION_COLOR_SWATCHES.map((swatch) => (
                          <button
                            key={swatch.color}
                            onClick={() =>
                              updateTransition(ti, {
                                color:
                                  transition.color === swatch.color
                                    ? undefined
                                    : swatch.color,
                              })
                            }
                            className={`w-5 h-5 rounded-full border-2 transition-all ${
                              transition.color === swatch.color
                                ? "border-gray-800 scale-110 ring-1 ring-gray-300"
                                : "border-transparent hover:border-gray-400"
                            }`}
                            style={{ backgroundColor: swatch.color }}
                            title={swatch.label}
                          />
                        ))}
                        <input
                          type="color"
                          value={transition.color || "#6b7280"}
                          onChange={(e) =>
                            updateTransition(ti, { color: e.target.value })
                          }
                          className="w-5 h-5 rounded cursor-pointer border border-gray-300 ml-1"
                          title="Custom color"
                        />
                      </div>
                    </div>

                    {/* Line style */}
                    <div>
                      <label className="block text-[10px] font-medium text-gray-500 mb-0.5">
                        Line Style
                      </label>
                      <div className="flex items-center gap-1">
                        {(
                          [
                            "solid",
                            "dashed",
                            "dotted",
                            "dash-dot",
                          ] as LineStyle[]
                        ).map((style) => {
                          const effectiveStyle =
                            transition.line_style || "solid";
                          const isActive = effectiveStyle === style;
                          const dashArrays: Record<LineStyle, string> = {
                            solid: "",
                            dashed: "6,4",
                            dotted: "2,3",
                            "dash-dot": "8,4,2,4",
                          };
                          const labels: Record<LineStyle, string> = {
                            solid: "Solid",
                            dashed: "Dashed",
                            dotted: "Dotted",
                            "dash-dot": "Dash-dot",
                          };
                          return (
                            <button
                              key={style}
                              onClick={() =>
                                updateTransition(ti, {
                                  line_style:
                                    style === "solid" ? undefined : style,
                                })
                              }
                              className={`flex items-center justify-center h-6 px-1.5 rounded border transition-all ${
                                isActive
                                  ? "border-gray-800 bg-gray-100"
                                  : "border-gray-200 hover:border-gray-400"
                              }`}
                              title={labels[style]}
                            >
                              <svg
                                width="28"
                                height="2"
                                className="overflow-visible"
                              >
                                <line
                                  x1="0"
                                  y1="1"
                                  x2="28"
                                  y2="1"
                                  stroke={
                                    transition.color ||
                                    EDGE_TYPE_COLORS[
                                      classifyTransitionWhen(transition.when)
                                    ] ||
                                    "#6b7280"
                                  }
                                  strokeWidth="2"
                                  strokeDasharray={dashArrays[style]}
                                />
                              </svg>
                            </button>
                          );
                        })}
                      </div>
                    </div>

                    {/* When condition */}
                    <div>
                      <label className="block text-[10px] font-medium text-gray-500 mb-0.5">
                        When (condition)
                      </label>
                      <input
                        type="text"
                        value={transition.when || ""}
                        onChange={(e) =>
                          updateTransition(ti, {
                            when: e.target.value || undefined,
                          })
                        }
                        className="w-full px-2 py-1 border border-gray-300 rounded text-xs font-mono focus:ring-1 focus:ring-blue-500 focus:border-blue-500"
                        placeholder="(empty = always)"
                      />
                      {/* Quick-set presets */}
                      <div className="flex gap-1 mt-1">
                        {(
                          [
                            "succeeded",
                            "failed",
                            "always",
                          ] as TransitionPreset[]
                        ).map((preset) => (
                          <button
                            key={preset}
                            onClick={() =>
                              updateTransition(ti, {
                                when: PRESET_WHEN[preset],
                              })
                            }
                            className="px-1.5 py-0.5 text-[9px] rounded border border-gray-200 hover:border-gray-400 text-gray-500 hover:text-gray-700 transition-colors"
                          >
                            {PRESET_LABELS[preset]}
                          </button>
                        ))}
                      </div>
                    </div>

                    {/* Do targets */}
                    <div>
                      <label className="block text-[10px] font-medium text-gray-500 mb-0.5">
                        Do (next tasks)
                      </label>
                      {(transition.do || []).map((targetName, di) => (
                        <div key={di} className="flex items-center gap-1 mb-1">
                          <ArrowRight className="w-3 h-3 text-gray-400 flex-shrink-0" />
                          <span className="text-xs font-mono text-gray-700 flex-1 truncate">
                            {targetName}
                          </span>
                          <button
                            onClick={() => removeDoTarget(ti, di)}
                            className="p-0.5 text-gray-400 hover:text-red-500"
                          >
                            <Trash2 className="w-2.5 h-2.5" />
                          </button>
                        </div>
                      ))}
                      {otherTaskNames.length > 0 && (
                        <SearchableSelect
                          value=""
                          onChange={(v) => {
                            if (v) {
                              addDoTarget(ti, String(v));
                            }
                          }}
                          options={otherTaskNames
                            .filter(
                              (name) => !(transition.do || []).includes(name),
                            )
                            .map((name) => ({
                              value: name,
                              label: name,
                            }))}
                          placeholder="+ Add target task..."
                        />
                      )}
                    </div>

                    {/* Publish directives */}
                    <div>
                      <label className="block text-[10px] font-medium text-gray-500 mb-0.5">
                        Publish (variables)
                      </label>
                      {(transition.publish || []).map((directive, pi) => {
                        const entries = Object.entries(directive);
                        if (entries.length === 0) return null;
                        const [key, value] = entries[0];
                        return (
                          <div
                            key={pi}
                            className="flex items-center gap-1 mb-1"
                          >
                            <input
                              type="text"
                              value={key}
                              onChange={(e) => {
                                const oldValue =
                                  Object.values(directive)[0] || "";
                                updatePublishDirective(ti, pi, {
                                  [e.target.value]: oldValue,
                                });
                              }}
                              className="flex-1 px-1.5 py-0.5 border border-gray-300 rounded text-[11px] font-mono focus:ring-1 focus:ring-blue-500"
                              placeholder="var_name"
                            />
                            <span className="text-gray-400 text-[10px]">=</span>
                            <input
                              type="text"
                              value={value}
                              onChange={(e) => {
                                const oldKey = Object.keys(directive)[0] || "";
                                updatePublishDirective(ti, pi, {
                                  [oldKey]: e.target.value,
                                });
                              }}
                              className="flex-1 px-1.5 py-0.5 border border-gray-300 rounded text-[11px] font-mono focus:ring-1 focus:ring-blue-500"
                              placeholder="{{ result() }}"
                            />
                            <button
                              onClick={() => removePublishDirective(ti, pi)}
                              className="p-0.5 text-gray-400 hover:text-red-500"
                            >
                              <Trash2 className="w-2.5 h-2.5" />
                            </button>
                          </div>
                        );
                      })}
                      <button
                        onClick={() => addPublishDirective(ti)}
                        className="flex items-center gap-0.5 text-[10px] text-blue-600 hover:text-blue-800 mt-0.5"
                      >
                        <Plus className="w-2.5 h-2.5" />
                        Add variable
                      </button>
                    </div>
                  </div>
                </div>
              );
            })}

            {/* Add transition buttons */}
            <div className="space-y-1.5 pt-1">
              <div className="flex gap-1.5">
                <button
                  onClick={() => addTransition("succeeded")}
                  className="flex-1 flex items-center justify-center gap-1 px-2 py-1.5 text-[10px] font-medium rounded border border-green-200 text-green-700 hover:bg-green-50 transition-colors"
                >
                  <Plus className="w-3 h-3" />
                  On Success
                </button>
                <button
                  onClick={() => addTransition("failed")}
                  className="flex-1 flex items-center justify-center gap-1 px-2 py-1.5 text-[10px] font-medium rounded border border-red-200 text-red-700 hover:bg-red-50 transition-colors"
                >
                  <Plus className="w-3 h-3" />
                  On Failure
                </button>
              </div>
              <button
                onClick={() => addTransition()}
                className="w-full flex items-center justify-center gap-1 px-2 py-1.5 text-[10px] font-medium rounded border border-gray-200 text-gray-600 hover:bg-gray-50 transition-colors"
              >
                <Plus className="w-3 h-3" />
                Custom transition
              </button>
            </div>
          </div>
        </CollapsibleSection>

        {/* Iteration Section */}
        <CollapsibleSection
          title="Iteration"
          sectionKey="iteration"
          expanded={expandedSections.has("iteration")}
          onToggle={toggleSection}
        >
          <div className="space-y-3">
            <label className="flex items-start gap-2">
              <input
                type="checkbox"
                checked={Boolean(task.iterate_cache)}
                onChange={(e) => {
                  if (e.target.checked) {
                    setLocalWithItems("");
                    update({
                      with_items: undefined,
                      iterate_cache: {
                        owner_type: "pack",
                        owner_ref: task.action.split(".")[0] || undefined,
                        namespace: "",
                        generation: "active",
                        page_size: 100,
                        require_fresh: false,
                      },
                    });
                  } else {
                    update({
                      iterate_cache: undefined,
                      batch_size: undefined,
                      concurrency: undefined,
                    });
                  }
                }}
                className="mt-0.5 h-3.5 w-3.5 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
              />
              <span>
                <span className="block text-xs font-medium text-gray-700">
                  Iterate Cache
                </span>
                <span className="block text-[10px] text-gray-400">
                  Fetch a cache generation in pages and process its records.
                </span>
              </span>
            </label>

            {task.iterate_cache && (
              <div className="space-y-3 rounded border border-blue-100 bg-blue-50/40 p-2.5">
                <div>
                  <label className="block text-xs font-medium text-gray-700 mb-1">
                    Owner Type
                  </label>
                  <select
                    value={task.iterate_cache.owner_type}
                    onChange={(e) =>
                      updateIterateCache({
                        owner_type: e.target.value as NonNullable<
                          WorkflowTask["iterate_cache"]
                        >["owner_type"],
                        owner_ref: undefined,
                      })
                    }
                    className="w-full px-2.5 py-1.5 border border-gray-300 rounded bg-white text-xs focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                  >
                    <option value="system">System</option>
                    <option value="identity">Current identity</option>
                    <option value="pack">Pack</option>
                    <option value="action">Action</option>
                    <option value="sensor">Sensor</option>
                  </select>
                </div>

                <div>
                  <label className="block text-xs font-medium text-gray-700 mb-1">
                    Owner Reference
                  </label>
                  <input
                    type="text"
                    value={task.iterate_cache.owner_ref || ""}
                    disabled={
                      task.iterate_cache.owner_type === "system" ||
                      task.iterate_cache.owner_type === "identity"
                    }
                    onChange={(e) =>
                      updateIterateCache({
                        owner_ref: e.target.value || undefined,
                      })
                    }
                    className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-xs font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500 disabled:bg-gray-100 disabled:text-gray-400 disabled:cursor-not-allowed"
                    placeholder="pack, action, or sensor ref"
                  />
                </div>

                <div>
                  <label className="block text-xs font-medium text-gray-700 mb-1">
                    Namespace
                  </label>
                  <input
                    type="text"
                    value={task.iterate_cache.namespace}
                    onChange={(e) =>
                      updateIterateCache({ namespace: e.target.value })
                    }
                    className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-xs font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                    placeholder="inventory or {{ parameters.namespace }}"
                  />
                </div>

                <div>
                  <label className="block text-xs font-medium text-gray-700 mb-1">
                    Generation
                  </label>
                  <input
                    type="text"
                    value={task.iterate_cache.generation}
                    onChange={(e) =>
                      updateIterateCache({ generation: e.target.value })
                    }
                    className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-xs font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                    placeholder="active, generation ID, or {{ expression }}"
                  />
                  <p className="text-[10px] text-gray-400 mt-0.5">
                    Defaults to the active generation at task start.
                  </p>
                </div>

                <div>
                  <label className="block text-xs font-medium text-gray-700 mb-1">
                    Page Size
                  </label>
                  <input
                    type="number"
                    value={task.iterate_cache.page_size}
                    onChange={(e) =>
                      updateIterateCache({
                        page_size: e.target.value
                          ? parseInt(e.target.value)
                          : 0,
                      })
                    }
                    className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                    min={1}
                    max={1000}
                  />
                  <p className="text-[10px] text-gray-400 mt-0.5">
                    Records fetched per cache request. Independent of Batch
                    Size.
                  </p>
                </div>

                <label className="flex items-center gap-2 text-xs font-medium text-gray-700">
                  <input
                    type="checkbox"
                    checked={task.iterate_cache.require_fresh}
                    onChange={(e) =>
                      updateIterateCache({ require_fresh: e.target.checked })
                    }
                    className="h-3.5 w-3.5 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                  />
                  Require a fresh generation
                </label>
              </div>
            )}

            <div>
              <label
                className={`block text-xs font-medium mb-1 ${task.iterate_cache ? "text-gray-400" : "text-gray-700"}`}
              >
                With Items
              </label>
              <input
                type="text"
                value={localWithItems}
                disabled={Boolean(task.iterate_cache)}
                onChange={(e) => setLocalWithItems(e.target.value)}
                onBlur={() =>
                  update({ with_items: localWithItems.trim() || undefined })
                }
                className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-xs font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500 disabled:bg-gray-100 disabled:text-gray-400 disabled:cursor-not-allowed"
                placeholder="{{ parameters.items }}"
              />
              <p className="text-[10px] text-gray-400 mt-0.5">
                Template expression resolving to a list for iteration.
              </p>
            </div>

            <div>
              <label
                className={`block text-xs font-medium mb-1 ${localWithItems || task.iterate_cache ? "text-gray-700" : "text-gray-400"}`}
              >
                Batch Size
              </label>
              <input
                type="number"
                value={task.batch_size || ""}
                disabled={!localWithItems && !task.iterate_cache}
                onChange={(e) =>
                  update({
                    batch_size: e.target.value
                      ? parseInt(e.target.value)
                      : undefined,
                  })
                }
                className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500 disabled:bg-gray-100 disabled:text-gray-400 disabled:cursor-not-allowed"
                placeholder="Process all at once"
                min={1}
              />
              <p className="text-[10px] text-gray-400 mt-0.5">
                Items per execution. For cache iteration, size 1 exposes item as
                an object; greater than 1 exposes item as an array.
              </p>
            </div>
            <div>
              <label
                className={`block text-xs font-medium mb-1 ${localWithItems || task.iterate_cache ? "text-gray-700" : "text-gray-400"}`}
              >
                Concurrency
              </label>
              <input
                type="number"
                value={task.concurrency || ""}
                disabled={!localWithItems && !task.iterate_cache}
                onChange={(e) =>
                  update({
                    concurrency: e.target.value
                      ? parseInt(e.target.value)
                      : undefined,
                  })
                }
                className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500 disabled:bg-gray-100 disabled:text-gray-400 disabled:cursor-not-allowed"
                placeholder="No limit"
                min={1}
              />
            </div>
          </div>
        </CollapsibleSection>

        {/* Delay, Retry & Timeout Section */}
        <CollapsibleSection
          title="Delay, Retry & Timeout"
          sectionKey="retry"
          expanded={expandedSections.has("retry")}
          onToggle={toggleSection}
        >
          <div className="space-y-3">
            {/* Delay */}
            <div>
              <label className="block text-xs font-medium text-gray-700 mb-1">
                Delay (seconds)
              </label>
              <input
                type="number"
                value={localDelay}
                onChange={(e) => setLocalDelay(e.target.value)}
                onBlur={() =>
                  update({
                    delay: localDelay ? parseInt(localDelay) : undefined,
                  })
                }
                className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                placeholder="No delay"
                min={1}
              />
              <p className="text-[10px] text-gray-400 mt-0.5">
                Number of seconds to wait before executing this task.
              </p>
            </div>

            {/* Timeout */}
            <div>
              <label className="block text-xs font-medium text-gray-700 mb-1">
                Timeout (seconds)
              </label>
              <input
                type="number"
                value={localTimeout}
                onChange={(e) => setLocalTimeout(e.target.value)}
                onBlur={() =>
                  update({
                    timeout: localTimeout ? parseInt(localTimeout) : undefined,
                  })
                }
                className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                placeholder="No timeout"
                min={1}
              />
            </div>

            <RetryEditor
              retry={task.retry}
              onChange={(retry) => update({ retry })}
            />
          </div>
        </CollapsibleSection>

        {/* Join Section */}
        <CollapsibleSection
          title="Join"
          sectionKey="join"
          expanded={expandedSections.has("join")}
          onToggle={toggleSection}
        >
          <div className="space-y-2">
            <p className="text-[10px] text-gray-400">
              If multiple tasks transition to this task, specify how many must
              complete before this task runs.
            </p>
            <div>
              <label className="block text-xs font-medium text-gray-700 mb-1">
                Join Count
              </label>
              <input
                type="number"
                value={task.join || ""}
                onChange={(e) =>
                  update({
                    join: e.target.value ? parseInt(e.target.value) : undefined,
                  })
                }
                className="w-full px-2.5 py-1.5 border border-gray-300 rounded text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
                placeholder="No join (runs immediately)"
                min={1}
              />
              <p className="text-[10px] text-gray-400 mt-0.5">
                Set to the number of inbound tasks, or leave empty for no
                barrier.
              </p>
            </div>
          </div>
        </CollapsibleSection>
      </div>
    </div>
  );
}

/**
 * Collapsible section wrapper
 */
function CollapsibleSection({
  title,
  sectionKey,
  expanded,
  onToggle,
  children,
}: {
  title: string;
  sectionKey: string;
  expanded: boolean;
  onToggle: (key: string) => void;
  children: React.ReactNode;
}) {
  return (
    <div className="border-b border-gray-100">
      <button
        onClick={() => onToggle(sectionKey)}
        className="w-full flex items-center gap-2 px-4 py-2.5 hover:bg-gray-50 transition-colors text-left"
      >
        {expanded ? (
          <ChevronDown className="w-3.5 h-3.5 text-gray-500 flex-shrink-0" />
        ) : (
          <ChevronRight className="w-3.5 h-3.5 text-gray-500 flex-shrink-0" />
        )}
        <span className="text-xs font-semibold text-gray-700 uppercase tracking-wider">
          {title}
        </span>
      </button>
      {expanded && <div className="px-4 pb-3">{children}</div>}
    </div>
  );
}

/**
 * Retry configuration editor
 */
function RetryEditor({
  retry,
  onChange,
}: {
  retry?: RetryConfig;
  onChange: (retry: RetryConfig | undefined) => void;
}) {
  if (!retry) {
    return (
      <button
        onClick={() => onChange({ count: 3, delay: 5, backoff: "constant" })}
        className="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-800"
      >
        <Plus className="w-3 h-3" />
        Add retry configuration
      </button>
    );
  }

  return (
    <div className="border border-gray-200 rounded p-2.5 bg-gray-50 space-y-2">
      <div className="flex items-center justify-between mb-1">
        <span className="text-xs font-medium text-gray-700">Retry Config</span>
        <button
          onClick={() => onChange(undefined)}
          className="p-0.5 text-gray-400 hover:text-red-500"
          title="Remove retry config"
        >
          <Trash2 className="w-3 h-3" />
        </button>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">
            Count
          </label>
          <input
            type="number"
            value={retry.count}
            onChange={(e) =>
              onChange({ ...retry, count: parseInt(e.target.value) || 1 })
            }
            className="w-full px-2 py-1 border border-gray-300 rounded text-xs focus:ring-1 focus:ring-blue-500"
            min={1}
            max={100}
          />
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">
            Delay (s)
          </label>
          <input
            type="number"
            value={retry.delay}
            onChange={(e) =>
              onChange({ ...retry, delay: parseInt(e.target.value) || 0 })
            }
            className="w-full px-2 py-1 border border-gray-300 rounded text-xs focus:ring-1 focus:ring-blue-500"
            min={0}
          />
        </div>
      </div>

      <div>
        <label className="block text-[10px] text-gray-500 mb-0.5">
          Backoff Strategy
        </label>
        <select
          value={retry.backoff || "constant"}
          onChange={(e) =>
            onChange({
              ...retry,
              backoff: e.target.value as "constant" | "linear" | "exponential",
            })
          }
          className="w-full px-2 py-1 border border-gray-300 rounded text-xs focus:ring-1 focus:ring-blue-500"
        >
          <option value="constant">Constant</option>
          <option value="linear">Linear</option>
          <option value="exponential">Exponential</option>
        </select>
      </div>

      {(retry.backoff === "exponential" || retry.backoff === "linear") && (
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">
            Max Delay (s)
          </label>
          <input
            type="number"
            value={retry.max_delay || ""}
            onChange={(e) =>
              onChange({
                ...retry,
                max_delay: e.target.value
                  ? parseInt(e.target.value)
                  : undefined,
              })
            }
            className="w-full px-2 py-1 border border-gray-300 rounded text-xs focus:ring-1 focus:ring-blue-500"
            placeholder="No maximum"
            min={1}
          />
        </div>
      )}
    </div>
  );
}
