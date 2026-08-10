import { useState, useCallback, useMemo, useEffect } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useQueries } from "@tanstack/react-query";
import {
  ArrowLeft,
  Save,
  Play,
  AlertTriangle,
  FileCode,
  Code,
  LayoutDashboard,
  X,
  Zap,
  Settings2,
  ExternalLink,
  Copy,
  Check,
  PanelLeftClose,
} from "lucide-react";
import SearchableSelect from "@/components/common/SearchableSelect";
import yaml from "js-yaml";
import type { WorkflowYamlDefinition } from "@/types/workflow";
import ActionPalette from "@/components/workflows/ActionPalette";
import WorkflowInputsPanel from "@/components/workflows/WorkflowInputsPanel";
import WorkflowCanvas from "@/components/workflows/WorkflowCanvas";
import type { EdgeHoverInfo } from "@/components/workflows/WorkflowEdges";
import TaskInspector from "@/components/workflows/TaskInspector";
import { useAction, useActions } from "@/hooks/useActions";
import { ActionsService } from "@/api";
import { usePacks } from "@/hooks/usePacks";
import { useRequestExecution } from "@/hooks/useExecutions";
import RunWorkflowModal from "@/components/workflows/RunWorkflowModal";
import type { ParamSchema } from "@/components/common/ParamSchemaForm";
import { useWorkflow } from "@/hooks/useWorkflows";
import {
  useSaveWorkflowFile,
  useUpdateWorkflowFile,
} from "@/hooks/useWorkflows";
import type {
  WorkflowTask,
  WorkflowBuilderState,
  PaletteAction,
  TransitionPreset,
} from "@/types/workflow";
import {
  generateUniqueTaskName,
  generateTaskId,
  builderStateToDefinition,
  builderStateToGraph,
  builderStateToActionYaml,
  definitionToBuilderState,
  validateWorkflow,
  addTransitionTarget,
  removeTaskFromTransitions,
  renameTaskInTransitions,
  findStartingTaskIds,
} from "@/types/workflow";

const INITIAL_STATE: WorkflowBuilderState = {
  name: "",
  label: "",
  description: "",
  version: "1.0.0",
  packRef: "",
  referenceVisibility: "public",
  referenceAllowedPackRefs: [],
  parameters: {},
  output: {},
  outputMap: {},
  vars: {},
  tasks: [],
  tags: [],
  cancellationPolicy: "allow_finish",
};

const ACTIONS_SIDEBAR_WIDTH = 256;
const WORKFLOW_OPTIONS_DEFAULT_WIDTH = 360;
const WORKFLOW_OPTIONS_STORAGE_KEY = "workflow-builder-options-width";

export default function WorkflowBuilderPage() {
  const navigate = useNavigate();
  const { ref: editRef } = useParams<{ ref?: string }>();
  const isEditing = !!editRef;

  // Data fetching
  const { data: packsData } = usePacks({ pageSize: 100 });
  const { data: existingWorkflow, isLoading: workflowLoading } = useWorkflow(
    editRef || "",
  );
  const { data: existingAction, isLoading: actionLoading } = useAction(
    editRef || "",
  );

  // Mutations
  const saveWorkflowFile = useSaveWorkflowFile();
  const updateWorkflowFile = useUpdateWorkflowFile();
  const requestExecution = useRequestExecution();

  // Builder state
  const [state, setState] = useState<WorkflowBuilderState>(INITIAL_STATE);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [showErrors, setShowErrors] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);
  const [showRunModal, setShowRunModal] = useState(false);
  const [yamlCopied, setYamlCopied] = useState(false);
  const [initialized, setInitialized] = useState(false);
  const [showYamlPreview, setShowYamlPreview] = useState(false);
  const [sidebarTab, setSidebarTab] = useState<"actions" | "inputs">("actions");
  const [workflowOptionsWidth, setWorkflowOptionsWidth] = useState<number>(
    () => {
      if (typeof window === "undefined") {
        return WORKFLOW_OPTIONS_DEFAULT_WIDTH;
      }
      const saved = window.localStorage.getItem(WORKFLOW_OPTIONS_STORAGE_KEY);
      const parsed = saved ? Number(saved) : NaN;
      return Number.isFinite(parsed) ? parsed : WORKFLOW_OPTIONS_DEFAULT_WIDTH;
    },
  );
  const [highlightedTransition, setHighlightedTransition] = useState<{
    taskId: string;
    transitionIndex: number;
  } | null>(null);
  const [isResizingSidebar, setIsResizingSidebar] = useState(false);

  // Start-node warning toast state
  const [startWarningVisible, setStartWarningVisible] = useState(false);
  const [startWarningDismissed, setStartWarningDismissed] = useState(false);
  const [showSaveConfirm, setShowSaveConfirm] = useState(false);
  const [prevWarningKey, setPrevWarningKey] = useState<string | null>(null);
  const [justInitialized, setJustInitialized] = useState(false);

  const { data: actionsData, isLoading: actionsLoading } = useActions({
    pageSize: 200,
    referencingPackRef: state.packRef || undefined,
  });

  const handleEdgeClick = useCallback(
    (info: EdgeHoverInfo | null) => {
      if (info) {
        // Select the source task so TaskInspector opens for it
        setSelectedTaskId(info.taskId);
        setHighlightedTransition(info);
      } else {
        setHighlightedTransition(null);
      }
    },
    [setSelectedTaskId],
  );

  // Initialize state from existing workflow (edit mode)
  if (
    isEditing &&
    existingWorkflow &&
    !initialized &&
    !workflowLoading &&
    !actionLoading
  ) {
    const workflow = existingWorkflow.data;
    const action = existingAction?.data;
    if (workflow) {
      // Extract name from ref (e.g., "pack.name" -> "name")
      const refParts = workflow.ref.split(".");
      const name =
        refParts.length > 1 ? refParts.slice(1).join(".") : workflow.ref;

      const defn = workflow.definition as Record<string, unknown> | undefined;
      const builderState = definitionToBuilderState(
        {
          ref: workflow.ref,
          label: workflow.label,
          description: workflow.description || undefined,
          version: workflow.version,
          parameters: workflow.param_schema || undefined,
          output: workflow.out_schema || undefined,
          vars: (defn?.vars as Record<string, unknown>) || undefined,
          tasks: (defn?.tasks as WorkflowYamlDefinition["tasks"]) || [],
          output_map: (defn?.output_map as Record<string, string>) || undefined,
          tags: workflow.tags,
          cancellation_policy:
            (defn?.cancellation_policy as
              "allow_finish" | "cancel_running" | undefined) || undefined,
        },
        workflow.pack_ref,
        name,
      );
      builderState.referenceVisibility =
        action?.reference_visibility ?? "public";
      builderState.referenceAllowedPackRefs =
        action?.reference_allowed_pack_refs ?? [];
      setState(builderState);
      setInitialized(true);
      setJustInitialized(true);
    }
  }

  // Derived data
  const paletteActions: PaletteAction[] = useMemo(() => {
    const actions = (actionsData?.items || []) as Array<{
      id: number;
      ref: string;
      label: string;
      description?: string;
      pack_ref: string;
    }>;
    return actions.map((a) => ({
      id: a.id,
      ref: a.ref,
      label: a.label,
      description: a.description || "",
      pack_ref: a.pack_ref,
    }));
  }, [actionsData]);

  // Fetch full action details for every unique action ref used in the workflow.
  // React Query caches each response, so repeated refs don't cause extra requests.
  const uniqueActionRefs = useMemo(() => {
    const refs = new Set<string>();
    for (const task of state.tasks) {
      if (task.action) refs.add(task.action);
    }
    return [...refs];
  }, [state.tasks]);

  const actionDetailQueries = useQueries({
    queries: uniqueActionRefs.map((ref) => ({
      queryKey: ["actions", ref],
      queryFn: () => ActionsService.getAction({ ref }),
      staleTime: 30_000,
      enabled: !!ref,
    })),
  });

  // Build action schema map from individually-fetched action details
  const actionSchemaMap = useMemo(() => {
    const map = new Map<string, Record<string, unknown> | null>();
    for (let i = 0; i < uniqueActionRefs.length; i++) {
      const query = actionDetailQueries[i];
      if (query.data?.data) {
        map.set(
          uniqueActionRefs[i],
          (query.data.data.param_schema as Record<string, unknown>) || null,
        );
      }
    }
    return map;
  }, [uniqueActionRefs, actionDetailQueries]);

  const packs = useMemo(() => {
    return (packsData?.items || []) as Array<{
      id: number;
      ref: string;
      label: string;
    }>;
  }, [packsData]);

  const selectedTask = useMemo(
    () => state.tasks.find((t) => t.id === selectedTaskId) || null,
    [state.tasks, selectedTaskId],
  );

  const allTaskNames = useMemo(
    () => state.tasks.map((t) => t.name),
    [state.tasks],
  );

  const startingTaskIds = useMemo(
    () => findStartingTaskIds(state.tasks),
    [state.tasks],
  );

  const startNodeWarning = useMemo(() => {
    if (state.tasks.length === 0) return null;
    const count = startingTaskIds.size;
    if (count === 0)
      return {
        level: "error" as const,
        message:
          "No starting tasks found — every task is a target of another transition, so the workflow has no entry point.",
      };
    if (count > 1)
      return {
        level: "warn" as const,
        message: `${count} starting tasks found (${state.tasks
          .filter((t) => startingTaskIds.has(t.id))
          .map((t) => `"${t.name}"`)
          .join(", ")}). Workflows typically have a single entry point.`,
      };
    return null;
  }, [state.tasks, startingTaskIds]);

  const getMaxWorkflowOptionsWidth = useCallback(() => {
    if (typeof window === "undefined") {
      return WORKFLOW_OPTIONS_DEFAULT_WIDTH;
    }
    return Math.max(ACTIONS_SIDEBAR_WIDTH, Math.floor(window.innerWidth * 0.5));
  }, []);

  const clampWorkflowOptionsWidth = useCallback(
    (width: number) =>
      Math.min(
        Math.max(Math.round(width), ACTIONS_SIDEBAR_WIDTH),
        getMaxWorkflowOptionsWidth(),
      ),
    [getMaxWorkflowOptionsWidth],
  );

  useEffect(() => {
    const handleResize = () => {
      setWorkflowOptionsWidth((prev) => clampWorkflowOptionsWidth(prev));
    };

    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, [clampWorkflowOptionsWidth]);

  useEffect(() => {
    window.localStorage.setItem(
      WORKFLOW_OPTIONS_STORAGE_KEY,
      String(workflowOptionsWidth),
    );
  }, [workflowOptionsWidth]);

  useEffect(() => {
    if (!isResizingSidebar) return;

    const handleMouseMove = (event: MouseEvent) => {
      setWorkflowOptionsWidth(clampWorkflowOptionsWidth(event.clientX));
    };

    const handleMouseUp = () => {
      setIsResizingSidebar(false);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };

    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", handleMouseUp);

    return () => {
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", handleMouseUp);
    };
  }, [isResizingSidebar, clampWorkflowOptionsWidth]);

  // Render-phase state adjustment: detect warning key changes for immediate
  // show/hide without refs or synchronous setState inside effects.
  const warningKey = startNodeWarning
    ? `${startNodeWarning.level}:${startingTaskIds.size}`
    : null;

  if (warningKey !== prevWarningKey) {
    setPrevWarningKey(warningKey);

    if (!warningKey) {
      // Condition resolved → immediately hide and allow future warnings
      if (startWarningVisible) setStartWarningVisible(false);
      if (startWarningDismissed) setStartWarningDismissed(false);
    } else if (justInitialized) {
      // Loaded from persistent storage with a problem → show immediately
      if (!startWarningVisible) setStartWarningVisible(true);
      setJustInitialized(false);
    }
  }

  // Debounce timer: starts a 15-second countdown for non-initial warning
  // appearances. The timer callback is async so it satisfies React's rules.
  useEffect(() => {
    if (!startNodeWarning || startWarningVisible || startWarningDismissed) {
      return;
    }

    const timer = setTimeout(() => {
      setStartWarningVisible(true);
    }, 15_000);

    return () => clearTimeout(timer);
  }, [startNodeWarning, startWarningVisible, startWarningDismissed]);

  // State updaters
  const updateMetadata = useCallback(
    (updates: Partial<WorkflowBuilderState>) => {
      setState((prev) => ({ ...prev, ...updates }));
      setSaveSuccess(false);
      setSaveError(null);
    },
    [],
  );

  const handleAddTaskFromPalette = useCallback(
    (action: PaletteAction) => {
      // Generate a task name from the action ref
      const baseName = action.ref.split(".").pop() || "task";
      const name = generateUniqueTaskName(state.tasks, baseName);

      // Position below existing tasks
      let maxY = 0;
      for (const task of state.tasks) {
        if (task.position.y > maxY) {
          maxY = task.position.y;
        }
      }

      const newTask: WorkflowTask = {
        id: generateTaskId(),
        name,
        action: action.ref,
        input: {},
        position: {
          x: 300,
          y: state.tasks.length === 0 ? 60 : maxY + 160,
        },
      };

      setState((prev) => ({
        ...prev,
        tasks: [...prev.tasks, newTask],
      }));
      setSelectedTaskId(newTask.id);
      setSaveSuccess(false);
    },
    [state.tasks],
  );

  const handleAddTask = useCallback((task: WorkflowTask) => {
    setState((prev) => ({
      ...prev,
      tasks: [...prev.tasks, task],
    }));
    setSaveSuccess(false);
  }, []);

  const handleUpdateTask = useCallback(
    (taskId: string, updates: Partial<WorkflowTask>) => {
      setState((prev) => {
        // Detect a name change so we can propagate it to transitions
        const oldName =
          updates.name !== undefined
            ? prev.tasks.find((t) => t.id === taskId)?.name
            : undefined;
        const newName = updates.name;
        const isRename =
          oldName !== undefined && newName !== undefined && oldName !== newName;

        return {
          ...prev,
          tasks: prev.tasks.map((t) => {
            if (t.id === taskId) {
              // Apply the updates, then also fix self-referencing transitions
              const merged = { ...t, ...updates };
              if (isRename) {
                const updatedNext = renameTaskInTransitions(
                  merged.next,
                  oldName,
                  newName,
                );
                if (updatedNext !== merged.next) {
                  return { ...merged, next: updatedNext };
                }
              }
              return merged;
            }
            // Update transition `do` lists that reference the old name
            if (isRename) {
              const updatedNext = renameTaskInTransitions(
                t.next,
                oldName,
                newName,
              );
              if (updatedNext !== t.next) {
                return { ...t, next: updatedNext };
              }
            }
            return t;
          }),
        };
      });
      setSaveSuccess(false);
    },
    [],
  );

  const handleDeleteTask = useCallback(
    (taskId: string) => {
      const taskToDelete = state.tasks.find((t) => t.id === taskId);
      if (!taskToDelete) return;

      setState((prev) => ({
        ...prev,
        tasks: prev.tasks
          .filter((t) => t.id !== taskId)
          .map((t) => {
            // Clean up any transitions that reference the deleted task
            const cleanedNext = removeTaskFromTransitions(
              t.next,
              taskToDelete.name,
            );
            if (cleanedNext !== t.next) {
              return { ...t, next: cleanedNext };
            }
            return t;
          }),
      }));

      if (selectedTaskId === taskId) {
        setSelectedTaskId(null);
      }
      setSaveSuccess(false);
    },
    [state.tasks, selectedTaskId],
  );

  const handleSetConnection = useCallback(
    (fromTaskId: string, preset: TransitionPreset, toTaskName: string) => {
      setState((prev) => ({
        ...prev,
        tasks: prev.tasks.map((t) => {
          if (t.id !== fromTaskId) return t;
          const next = addTransitionTarget(t, preset, toTaskName);
          return { ...t, next };
        }),
      }));
      setSaveSuccess(false);
    },
    [],
  );

  const doSave = useCallback(async () => {
    // Validate
    const errors = validateWorkflow(state, actionSchemaMap);
    setValidationErrors(errors);

    if (errors.length > 0) {
      setShowErrors(true);
      return false;
    }

    const definition = builderStateToDefinition(state, actionSchemaMap);

    try {
      setSaveError(null);

      if (isEditing && editRef) {
        await updateWorkflowFile.mutateAsync({
          workflowRef: editRef,
          data: {
            name: state.name,
            label: state.label,
            description: state.description || undefined,
            version: state.version,
            pack_ref: state.packRef,
            reference_visibility: state.referenceVisibility,
            reference_allowed_pack_refs:
              state.referenceVisibility === "restricted"
                ? state.referenceAllowedPackRefs
                : [],
            definition,
            param_schema:
              Object.keys(state.parameters).length > 0
                ? state.parameters
                : undefined,
            out_schema:
              Object.keys(state.output).length > 0 ? state.output : undefined,
            tags: state.tags.length > 0 ? state.tags : undefined,
          },
        });
      } else {
        const fileData = {
          name: state.name,
          label: state.label,
          description: state.description || undefined,
          version: state.version,
          pack_ref: state.packRef,
          reference_visibility: state.referenceVisibility,
          reference_allowed_pack_refs:
            state.referenceVisibility === "restricted"
              ? state.referenceAllowedPackRefs
              : [],
          definition,
          param_schema:
            Object.keys(state.parameters).length > 0
              ? state.parameters
              : undefined,
          out_schema:
            Object.keys(state.output).length > 0 ? state.output : undefined,
          tags: state.tags.length > 0 ? state.tags : undefined,
        };
        try {
          await saveWorkflowFile.mutateAsync(fileData);
        } catch (createErr: unknown) {
          const apiErr = createErr as { status?: number };
          if (apiErr?.status === 409) {
            // Workflow already exists — fall back to update
            const workflowRef = `${state.packRef}.${state.name}`;
            await updateWorkflowFile.mutateAsync({
              workflowRef,
              data: fileData,
            });
          } else {
            throw createErr;
          }
        }
      }

      // After a successful first save, navigate to the edit URL so the
      // page transitions into edit mode (locks ref, uses update on next save).
      if (!isEditing) {
        const newRef = `${state.packRef}.${state.name}`;
        navigate(`/actions/workflows/${newRef}/edit`, { replace: true });
        return true;
      }

      setSaveSuccess(true);
      setTimeout(() => setSaveSuccess(false), 3000);

      return true; // indicate success
    } catch (err: unknown) {
      const error = err as { body?: { message?: string }; message?: string };
      const message =
        error?.body?.message || error?.message || "Failed to save workflow";
      setSaveError(message);
      return false; // indicate failure
    }
  }, [
    state,
    isEditing,
    editRef,
    saveWorkflowFile,
    updateWorkflowFile,
    actionSchemaMap,
    navigate,
  ]);

  // Check whether the workflow has any parameters defined
  const hasParameters = useMemo(
    () => Object.keys(state.parameters).length > 0,
    [state.parameters],
  );

  const handleRun = useCallback(async () => {
    setRunError(null);

    if (hasParameters) {
      // Open the modal so the user can review / override parameter values
      setShowRunModal(true);
      return;
    }

    // No parameters — save and execute immediately
    const saved = await doSave();
    if (!saved) return; // save failed — error already shown

    const actionRef = editRef || `${state.packRef}.${state.name}`;

    try {
      const response = await requestExecution.mutateAsync({
        actionRef,
        parameters: {},
      });
      const executionId = response.data.id;
      window.open(`/executions/${executionId}`, "_blank");
    } catch (err: unknown) {
      const error = err as { body?: { message?: string }; message?: string };
      const message =
        error?.body?.message || error?.message || "Failed to start execution";
      setRunError(message);
    }
  }, [
    hasParameters,
    doSave,
    editRef,
    state.packRef,
    state.name,
    requestExecution,
  ]);

  const handleSave = useCallback(() => {
    // If there's a start-node problem, show the toast immediately and
    // require confirmation before saving
    if (startNodeWarning) {
      setStartWarningVisible(true);
      setStartWarningDismissed(false);
      setShowSaveConfirm(true);
      return;
    }
    doSave();
  }, [startNodeWarning, doSave]);

  // YAML previews — two separate panels for the two-file model:
  // 1. Action YAML (ref, label, parameters, output, tags, workflow_file)
  // 2. Workflow YAML (version, vars, tasks, output_map — graph only)
  const actionYamlPreview = useMemo(() => {
    if (!showYamlPreview) return "";
    try {
      const actionDef = builderStateToActionYaml(state);
      return yaml.dump(actionDef, {
        indent: 2,
        lineWidth: 120,
        noRefs: true,
        sortKeys: false,
        quotingType: '"',
        forceQuotes: false,
      });
    } catch {
      return "# Error generating action YAML preview";
    }
  }, [state, showYamlPreview]);

  const workflowYamlPreview = useMemo(() => {
    if (!showYamlPreview) return "";
    try {
      const graphDef = builderStateToGraph(state, actionSchemaMap);
      return yaml.dump(graphDef, {
        indent: 2,
        lineWidth: 120,
        noRefs: true,
        sortKeys: false,
        quotingType: '"',
        forceQuotes: false,
      });
    } catch {
      return "# Error generating workflow YAML preview";
    }
  }, [state, showYamlPreview, actionSchemaMap]);

  const isSaving = saveWorkflowFile.isPending || updateWorkflowFile.isPending;
  const isExecuting = requestExecution.isPending;
  const sidebarWidth =
    sidebarTab === "inputs" ? workflowOptionsWidth : ACTIONS_SIDEBAR_WIDTH;

  if (isEditing && (workflowLoading || actionLoading)) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600" />
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Top toolbar */}
      <div className="flex-shrink-0 bg-white border-b border-gray-200 px-4 py-2.5">
        <div className="flex items-center justify-between">
          {/* Left section: Back + metadata */}
          <div className="flex items-center gap-3 flex-1 min-w-0">
            <button
              onClick={() =>
                navigate(isEditing ? `/actions/${editRef}` : "/actions")
              }
              className="p-1.5 rounded hover:bg-gray-100 text-gray-500 hover:text-gray-700 transition-colors flex-shrink-0"
              title={isEditing ? "Back to Workflow" : "Back to Actions"}
            >
              <ArrowLeft className="w-5 h-5" />
            </button>

            <div className="flex items-center gap-2 flex-1 min-w-0">
              {/* Pack selector */}
              <SearchableSelect
                value={state.packRef}
                onChange={(v) => updateMetadata({ packRef: String(v) })}
                options={packs.map((pack) => ({
                  value: pack.ref,
                  label: pack.ref,
                }))}
                placeholder="Pack..."
                className="max-w-[140px]"
                disabled={isEditing}
              />

              <span className="text-gray-400 text-lg font-light">/</span>

              {/* Workflow name */}
              <input
                type="text"
                value={state.name}
                onChange={(e) =>
                  updateMetadata({
                    name: e.target.value
                      .toLowerCase()
                      .replace(/[^a-z0-9_-]/g, "_"),
                  })
                }
                className={`px-2 py-1.5 border border-gray-300 rounded text-sm font-mono w-48 ${isEditing ? "bg-gray-100 cursor-not-allowed text-gray-500" : "focus:ring-2 focus:ring-blue-500 focus:border-blue-500"}`}
                placeholder="workflow_name"
                title="Use lowercase letters, numbers, underscores, or hyphens"
                disabled={isEditing}
              />

              <span className="text-gray-400 text-lg font-light">—</span>
              <span className="truncate text-sm text-gray-600">
                {state.label || "Untitled workflow"}
              </span>
            </div>
          </div>

          {/* Right section: Actions */}
          <div className="flex items-center gap-2 flex-shrink-0 ml-4">
            {/* Validation errors badge */}
            {validationErrors.length > 0 && (
              <button
                onClick={() => setShowErrors(!showErrors)}
                className="flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium text-amber-700 bg-amber-50 border border-amber-200 rounded hover:bg-amber-100 transition-colors"
              >
                <AlertTriangle className="w-3.5 h-3.5" />
                {validationErrors.length} issue
                {validationErrors.length !== 1 ? "s" : ""}
              </button>
            )}

            {/* Raw YAML / Visual mode toggle */}
            <div className="flex items-center bg-gray-100 rounded-lg p-0.5">
              <button
                onClick={() => setShowYamlPreview(false)}
                className={`flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium rounded-md transition-colors ${
                  !showYamlPreview
                    ? "bg-white text-gray-900 shadow-sm"
                    : "text-gray-500 hover:text-gray-700"
                }`}
                title="Visual builder"
              >
                <LayoutDashboard className="w-3.5 h-3.5" />
                Visual
              </button>
              <button
                onClick={() => setShowYamlPreview(true)}
                className={`flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium rounded-md transition-colors ${
                  showYamlPreview
                    ? "bg-white text-gray-900 shadow-sm"
                    : "text-gray-500 hover:text-gray-700"
                }`}
                title="Raw YAML view"
              >
                <Code className="w-3.5 h-3.5" />
                Raw YAML
              </button>
            </div>

            {/* Save success indicator */}
            {saveSuccess && (
              <span className="text-xs text-green-600 font-medium">
                ✓ Saved
              </span>
            )}

            {/* Save error indicator */}
            {saveError && (
              <span
                className="text-xs text-red-600 font-medium max-w-[200px] truncate"
                title={saveError}
              >
                ✗ {saveError}
              </span>
            )}

            {/* Run error indicator */}
            {runError && (
              <span
                className="text-xs text-red-600 font-medium max-w-[200px] truncate"
                title={runError}
              >
                ✗ {runError}
              </span>
            )}

            {/* Save button */}
            <button
              onClick={handleSave}
              disabled={isSaving}
              className="flex items-center gap-1.5 px-4 py-1.5 bg-blue-600 text-white text-sm font-medium rounded hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed transition-colors shadow-sm"
            >
              <Save className="w-4 h-4" />
              {isSaving ? "Saving..." : isEditing ? "Update" : "Save"}
            </button>

            {/* Run button */}
            <button
              onClick={handleRun}
              disabled={!isEditing || isSaving || isExecuting}
              title={
                !isEditing
                  ? "Save the workflow first to enable execution"
                  : "Save & run this workflow"
              }
              className="flex items-center gap-1.5 px-4 py-1.5 bg-green-600 text-white text-sm font-medium rounded hover:bg-green-700 disabled:opacity-50 disabled:cursor-not-allowed transition-colors shadow-sm"
            >
              {isExecuting ? (
                <>
                  <div className="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin" />
                  Running...
                </>
              ) : (
                <>
                  <Play className="w-4 h-4" />
                  Run
                  <ExternalLink className="w-3 h-3 opacity-60" />
                </>
              )}
            </button>
          </div>
        </div>
      </div>

      {/* Validation errors panel */}
      {showErrors && validationErrors.length > 0 && (
        <div className="flex-shrink-0 bg-amber-50 border-b border-amber-200 px-4 py-2">
          <div className="flex items-start gap-2">
            <AlertTriangle className="w-4 h-4 text-amber-600 mt-0.5 flex-shrink-0" />
            <div className="flex-1">
              <p className="text-xs font-medium text-amber-800 mb-1">
                Please fix the following issues before saving:
              </p>
              <ul className="text-xs text-amber-700 space-y-0.5">
                {validationErrors.map((error, index) => (
                  <li key={index}>• {error}</li>
                ))}
              </ul>
            </div>
            <button
              onClick={() => setShowErrors(false)}
              className="text-amber-400 hover:text-amber-600"
            >
              ×
            </button>
          </div>
        </div>
      )}

      {/* Main content area */}
      <div className="flex-1 flex overflow-hidden">
        {showYamlPreview ? (
          /* Raw YAML mode — two-panel view: Action YAML + Workflow YAML */
          <div className="flex-1 flex overflow-hidden">
            {/* Left panel: Action YAML */}
            <div className="w-2/5 flex flex-col overflow-hidden bg-gray-900 border-r border-gray-700">
              <div className="flex items-center gap-2 px-4 py-2 bg-gray-800 border-b border-gray-700 flex-shrink-0">
                <FileCode className="w-4 h-4 text-blue-400" />
                <span className="text-sm font-medium text-gray-300">
                  Action YAML
                </span>
                <span className="text-[10px] text-gray-500 ml-1">
                  actions/{state.name}.yaml
                </span>
                <div className="flex-1" />
                <button
                  onClick={() => {
                    navigator.clipboard.writeText(actionYamlPreview);
                  }}
                  className="flex items-center gap-1 px-2 py-1 text-xs text-gray-400 hover:text-gray-200 bg-gray-700 hover:bg-gray-600 rounded transition-colors"
                  title="Copy action YAML to clipboard"
                >
                  <Copy className="w-3.5 h-3.5" />
                  <span>Copy</span>
                </button>
              </div>
              <div className="px-4 py-2 bg-gray-800/50 border-b border-gray-700/50 flex-shrink-0">
                <p className="text-[10px] text-gray-500 leading-relaxed">
                  Defines the action identity, parameters, and output schema.
                  References the workflow file via{" "}
                  <code className="text-gray-400">workflow_file</code>.
                </p>
              </div>
              <pre className="flex-1 overflow-auto p-4 text-sm font-mono text-blue-300 whitespace-pre leading-relaxed">
                {actionYamlPreview}
              </pre>
            </div>

            {/* Right panel: Workflow YAML (graph only) */}
            <div className="flex-1 flex flex-col overflow-hidden bg-gray-900">
              <div className="flex items-center gap-2 px-4 py-2 bg-gray-800 border-b border-gray-700 flex-shrink-0">
                <FileCode className="w-4 h-4 text-green-400" />
                <span className="text-sm font-medium text-gray-300">
                  Workflow YAML
                </span>
                <span className="text-[10px] text-gray-500 ml-1">
                  actions/workflows/{state.name}.workflow.yaml
                </span>
                <div className="flex-1" />
                <button
                  onClick={() => {
                    navigator.clipboard
                      .writeText(workflowYamlPreview)
                      .then(() => {
                        setYamlCopied(true);
                        setTimeout(() => setYamlCopied(false), 2000);
                      });
                  }}
                  className="flex items-center gap-1 px-2 py-1 text-xs text-gray-400 hover:text-gray-200 bg-gray-700 hover:bg-gray-600 rounded transition-colors"
                  title="Copy workflow YAML to clipboard"
                >
                  {yamlCopied ? (
                    <>
                      <Check className="w-3.5 h-3.5 text-green-400" />
                      <span className="text-green-400">Copied</span>
                    </>
                  ) : (
                    <>
                      <Copy className="w-3.5 h-3.5" />
                      <span>Copy</span>
                    </>
                  )}
                </button>
              </div>
              <div className="px-4 py-2 bg-gray-800/50 border-b border-gray-700/50 flex-shrink-0">
                <p className="text-[10px] text-gray-500 leading-relaxed">
                  Execution graph only — tasks, transitions, variables. No
                  action-level metadata (those are in the action YAML).
                </p>
              </div>
              <pre className="flex-1 overflow-auto p-4 text-sm font-mono text-green-400 whitespace-pre leading-relaxed">
                {workflowYamlPreview}
              </pre>
            </div>
          </div>
        ) : (
          <>
            {/* Left sidebar: tabbed Actions / Workflow Options */}
            <div
              className="border-r border-gray-200 bg-gray-50 flex flex-col h-full overflow-hidden relative flex-shrink-0"
              style={{ width: sidebarWidth }}
            >
              {/* Tab header */}
              <div className="flex border-b border-gray-200 bg-white flex-shrink-0">
                <button
                  onClick={() => setSidebarTab("actions")}
                  className={`flex-1 flex items-center justify-center gap-1.5 px-3 py-2 text-xs font-medium transition-colors ${
                    sidebarTab === "actions"
                      ? "text-blue-600 border-b-2 border-blue-600 bg-blue-50/50"
                      : "text-gray-500 hover:text-gray-700 hover:bg-gray-50"
                  }`}
                >
                  <Zap className="w-3.5 h-3.5" />
                  Actions
                </button>
                <button
                  onClick={() => setSidebarTab("inputs")}
                  className={`flex-1 flex items-center justify-center gap-1.5 px-3 py-2 text-xs font-medium transition-colors ${
                    sidebarTab === "inputs"
                      ? "text-blue-600 border-b-2 border-blue-600 bg-blue-50/50"
                      : "text-gray-500 hover:text-gray-700 hover:bg-gray-50"
                  }`}
                >
                  <Settings2 className="w-3.5 h-3.5" />
                  Workflow Options
                  {Object.keys(state.parameters).length > 0 && (
                    <span className="text-[10px] bg-blue-100 text-blue-700 px-1.5 py-0.5 rounded-full">
                      {Object.keys(state.parameters).length}
                    </span>
                  )}
                </button>
              </div>

              {/* Tab content */}
              {sidebarTab === "actions" ? (
                <ActionPalette
                  actions={paletteActions}
                  isLoading={actionsLoading}
                  onAddTask={handleAddTaskFromPalette}
                />
              ) : (
                <WorkflowInputsPanel
                  label={state.label}
                  version={state.version}
                  description={state.description}
                  tags={state.tags}
                  referenceVisibility={state.referenceVisibility}
                  referenceAllowedPackRefs={state.referenceAllowedPackRefs}
                  cancellationPolicy={state.cancellationPolicy}
                  parameters={state.parameters}
                  output={state.output}
                  outputMap={state.outputMap}
                  onLabelChange={(label) => updateMetadata({ label })}
                  onVersionChange={(version) => updateMetadata({ version })}
                  onDescriptionChange={(description) =>
                    updateMetadata({ description })
                  }
                  onTagsChange={(tags) => updateMetadata({ tags })}
                  onReferenceVisibilityChange={(referenceVisibility) =>
                    updateMetadata({
                      referenceVisibility,
                      referenceAllowedPackRefs:
                        referenceVisibility === "restricted"
                          ? state.referenceAllowedPackRefs
                          : [],
                    })
                  }
                  onReferenceAllowedPackRefsChange={(
                    referenceAllowedPackRefs,
                  ) => updateMetadata({ referenceAllowedPackRefs })}
                  onCancellationPolicyChange={(cancellationPolicy) =>
                    updateMetadata({ cancellationPolicy })
                  }
                  onParametersChange={(parameters) =>
                    setState((prev) => ({ ...prev, parameters }))
                  }
                  onOutputChange={(output) =>
                    setState((prev) => ({ ...prev, output }))
                  }
                  onOutputMapChange={(outputMap) =>
                    setState((prev) => ({ ...prev, outputMap }))
                  }
                />
              )}

              {sidebarTab === "inputs" && (
                <div
                  className={`absolute top-0 right-0 h-full w-2 translate-x-1/2 cursor-col-resize group ${
                    isResizingSidebar ? "z-30" : "z-10"
                  }`}
                  onMouseDown={(event) => {
                    event.preventDefault();
                    setIsResizingSidebar(true);
                  }}
                  title="Resize workflow options panel"
                >
                  <div
                    className={`mx-auto h-full w-px transition-colors ${
                      isResizingSidebar
                        ? "bg-blue-500"
                        : "bg-transparent group-hover:bg-blue-300"
                    }`}
                  />
                  <div className="absolute top-3 right-0 -translate-y-1/2 translate-x-1/2 rounded-full border border-gray-200 bg-white p-1 text-gray-300 shadow-sm group-hover:text-blue-500">
                    <PanelLeftClose className="w-3 h-3" />
                  </div>
                </div>
              )}
            </div>

            {/* Center: Canvas */}
            <WorkflowCanvas
              tasks={state.tasks}
              selectedTaskId={selectedTaskId}
              onSelectTask={setSelectedTaskId}
              onUpdateTask={handleUpdateTask}
              onDeleteTask={handleDeleteTask}
              onAddTask={handleAddTask}
              onSetConnection={handleSetConnection}
              onEdgeClick={handleEdgeClick}
            />

            {/* Right: Task Inspector */}
            {selectedTask && (
              <TaskInspector
                task={selectedTask}
                allTaskNames={allTaskNames}
                availableActions={paletteActions}
                onUpdate={handleUpdateTask}
                onClose={() => setSelectedTaskId(null)}
                highlightTransitionIndex={
                  highlightedTransition?.taskId === selectedTask.id
                    ? highlightedTransition.transitionIndex
                    : null
                }
              />
            )}
          </>
        )}
      </div>

      {/* Floating start-node warning toast */}
      {startNodeWarning && startWarningVisible && (
        <div
          className="fixed top-20 left-1/2 -translate-x-1/2 z-50 animate-fade-in"
          style={{
            animation: "fadeInDown 0.25s ease-out both",
          }}
        >
          <div
            className={`flex items-center gap-2.5 px-4 py-2.5 rounded-lg shadow-lg border ${
              startNodeWarning.level === "error"
                ? "bg-red-50 border-red-300 text-red-800"
                : "bg-amber-50 border-amber-300 text-amber-800"
            }`}
            style={{ maxWidth: 520 }}
          >
            <AlertTriangle
              className={`w-4 h-4 flex-shrink-0 ${
                startNodeWarning.level === "error"
                  ? "text-red-500"
                  : "text-amber-500"
              }`}
            />
            <p className="text-xs font-medium flex-1">
              {startNodeWarning.message}
            </p>
            <button
              onClick={() => {
                setStartWarningVisible(false);
                setStartWarningDismissed(true);
              }}
              className={`p-0.5 rounded hover:bg-black/5 flex-shrink-0 ${
                startNodeWarning.level === "error"
                  ? "text-red-400 hover:text-red-600"
                  : "text-amber-400 hover:text-amber-600"
              }`}
            >
              <X className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>
      )}

      {/* Confirmation modal for saving with start-node warnings */}
      {showSaveConfirm && startNodeWarning && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center">
          {/* Backdrop */}
          <div
            className="absolute inset-0 bg-black/40"
            onClick={() => setShowSaveConfirm(false)}
          />
          {/* Modal */}
          <div className="relative bg-white rounded-lg shadow-xl border border-gray-200 p-6 max-w-md w-full mx-4">
            <div className="flex items-start gap-3">
              <div
                className={`p-2 rounded-full flex-shrink-0 ${
                  startNodeWarning.level === "error"
                    ? "bg-red-100"
                    : "bg-amber-100"
                }`}
              >
                <AlertTriangle
                  className={`w-5 h-5 ${
                    startNodeWarning.level === "error"
                      ? "text-red-600"
                      : "text-amber-600"
                  }`}
                />
              </div>
              <div className="flex-1">
                <h3 className="text-sm font-semibold text-gray-900 mb-1">
                  {startNodeWarning.level === "error"
                    ? "No starting tasks"
                    : "Multiple starting tasks"}
                </h3>
                <p className="text-xs text-gray-600 mb-4">
                  {startNodeWarning.message} Are you sure you want to save this
                  workflow?
                </p>
                <div className="flex items-center justify-end gap-2">
                  <button
                    onClick={() => setShowSaveConfirm(false)}
                    className="px-3 py-1.5 text-sm font-medium text-gray-700 bg-gray-100 rounded hover:bg-gray-200 transition-colors"
                  >
                    Cancel
                  </button>
                  <button
                    onClick={() => {
                      setShowSaveConfirm(false);
                      doSave();
                    }}
                    className="px-3 py-1.5 text-sm font-medium text-white bg-blue-600 rounded hover:bg-blue-700 transition-colors"
                  >
                    Save Anyway
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Run workflow modal (shown when workflow has parameters) */}
      {showRunModal && (
        <RunWorkflowModal
          actionRef={editRef || `${state.packRef}.${state.name}`}
          paramSchema={state.parameters as unknown as ParamSchema}
          label={state.label || undefined}
          onSave={doSave}
          onClose={() => setShowRunModal(false)}
        />
      )}

      {/* Inline style for fade-in animation */}
      <style>{`
          @keyframes fadeInDown {
            from {
              opacity: 0;
              transform: translate(-50%, -8px);
            }
            to {
              opacity: 1;
              transform: translate(-50%, 0);
            }
          }
        `}</style>
    </div>
  );
}
