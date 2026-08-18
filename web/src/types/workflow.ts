/**
 * Workflow Builder Types
 *
 * These types represent the client-side workflow builder state
 * and map to the backend workflow YAML format.
 *
 * Uses the Orquesta-style task transition model where each task has a `next`
 * list of transitions. Each transition specifies:
 *   - `when` — a condition expression (e.g., "{{ succeeded() }}", "{{ failed() }}")
 *   - `publish` — variables to publish into the workflow context
 *   - `do` — next tasks to invoke when the condition is met
 */

/** Position of a node on the canvas */
export interface NodePosition {
  x: number;
  y: number;
}

/**
 * A single task transition evaluated after task completion.
 *
 * Transitions are evaluated in order. When `when` is not defined,
 * the transition is unconditional (fires on any completion).
 */
/** Line style for transition edges */
export type LineStyle = "solid" | "dashed" | "dotted" | "dash-dot";

export interface TaskTransition {
  /** Condition expression (e.g., "{{ succeeded() }}", "{{ failed() }}") */
  when?: string;
  /** Variables to publish into the workflow context on this transition */
  publish?: PublishDirective[];
  /** Next tasks to invoke when transition criteria is met */
  do?: string[];
  /** Custom display label for the transition (overrides auto-derived label) */
  label?: string;
  /** Custom color for the transition edge (CSS color string, e.g., "#ff6600") */
  color?: string;
  /** Custom line style for the transition edge (overrides type-based default) */
  line_style?: LineStyle;
  /** Intermediate waypoints per target task (keyed by target task name) for edge routing */
  edge_waypoints?: Record<string, NodePosition[]>;
  /** Label position per target task as t-parameter (0–1) along the edge path */
  label_positions?: Record<string, number>;
}

/** A task node in the workflow builder */
export interface WorkflowTask {
  /** Unique ID for the builder (not persisted) */
  id: string;
  /** Task name (used in YAML) */
  name: string;
  /** Action reference (e.g., "core.echo") */
  action: string;
  /** Input parameters (template strings or values) */
  input: Record<string, unknown>;
  /** Permission set refs for this task's execution token; may be a template */
  permission_set_refs?: string | string[];
  /** Task transitions — evaluated in order after task completes */
  next?: TaskTransition[];
  /** Delay in seconds before executing this task */
  delay?: number;
  /** Retry configuration */
  retry?: RetryConfig;
  /** Timeout in seconds — literal integer or template expression resolving to an integer */
  timeout?: number | string;
  /** With-items iteration expression */
  with_items?: string;
  /** Iterate over pages from a cache generation */
  iterate_cache?: IterateCacheConfig;
  /** Items per execution for with-items or cache iteration */
  batch_size?: number;
  /** Concurrency limit for with-items or cache iteration */
  concurrency?: number;
  /** Join barrier count */
  join?: number;
  /** Visual position on canvas */
  position: NodePosition;
}

export type CacheOwnerType =
  "system" | "identity" | "pack" | "action" | "sensor";

/** Cache-backed task iteration configuration */
export interface IterateCacheConfig {
  owner_type: CacheOwnerType;
  owner_ref?: string;
  namespace: string;
  /** `active`, a generation ID, or a pure template expression */
  generation: string;
  page_size: number;
  require_fresh: boolean;
}

/** YAML shape permits fields with backend defaults to be omitted. */
export interface WorkflowYamlIterateCacheConfig {
  owner_type: CacheOwnerType;
  owner_ref?: string;
  namespace: string;
  generation?: string;
  page_size?: number;
  require_fresh?: boolean;
}

/** Retry configuration */
export interface RetryConfig {
  /** Number of retry attempts */
  count: number;
  /** Initial delay in seconds */
  delay: number;
  /** Backoff strategy */
  backoff?: "constant" | "linear" | "exponential";
  /** Maximum delay in seconds */
  max_delay?: number;
  /** Only retry on specific error conditions */
  on_error?: string;
}

/** Variable publishing directive */
export type PublishDirective = Record<string, string>;

/**
 * Transition handle presets for the visual builder.
 *
 * These map to common `when` expressions and provide a quick way
 * to create transitions without typing expressions manually.
 */
export type TransitionPreset = "succeeded" | "failed" | "always";

/** The `when` expression for each preset (undefined = unconditional) */
export const PRESET_WHEN: Record<TransitionPreset, string | undefined> = {
  succeeded: "{{ succeeded() }}",
  failed: "{{ failed() }}",
  always: undefined,
};

/** Human-readable labels for presets */
export const PRESET_LABELS: Record<TransitionPreset, string> = {
  succeeded: "On Success",
  failed: "On Failure",
  always: "Always",
};

/** Default edge colors for each preset */
export const PRESET_COLORS: Record<TransitionPreset, string> = {
  succeeded: "#22c55e", // green-500
  failed: "#ef4444", // red-500
  always: "#6b7280", // gray-500
};

export const PRESET_STYLES: Record<TransitionPreset, LineStyle> = {
  succeeded: "solid",
  failed: "dashed",
  always: "solid",
};

/**
 * Classify a `when` expression into an edge visual type.
 * Used for edge coloring and labeling.
 */
export type EdgeType = "success" | "failure" | "complete" | "custom";

/** Default colors for each EdgeType (mirrors PRESET_COLORS but keyed by EdgeType). */
export const EDGE_TYPE_COLORS: Record<EdgeType, string> = {
  success: "#22c55e", // green-500
  failure: "#ef4444", // red-500
  complete: "#6b7280", // gray-500 (unconditional / always)
  custom: "#8b5cf6", // violet-500
};

export function classifyTransitionWhen(when?: string): EdgeType {
  if (!when) return "complete"; // unconditional
  const lower = when.toLowerCase().replace(/\s+/g, "");
  if (lower.includes("succeeded()")) return "success";
  if (lower.includes("failed()")) return "failure";
  return "custom";
}

/** Human-readable short label for a `when` expression */
export function transitionLabel(when?: string, customLabel?: string): string {
  if (customLabel) return customLabel;
  if (!when) return "always";
  const lower = when.toLowerCase().replace(/\s+/g, "");
  if (lower.includes("succeeded()")) return "succeeded";
  if (lower.includes("failed()")) return "failed";
  // Truncate custom expressions for display
  if (when.length > 30) return when.slice(0, 27) + "...";
  return when;
}

/** An edge/connection between two tasks */
export interface WorkflowEdge {
  /** Source task ID */
  from: string;
  /** Target task ID */
  to: string;
  /** Target task name (stable key for waypoints) */
  toName: string;
  /** Visual type of transition (derived from `when`) */
  type: EdgeType;
  /** Label to display on the edge */
  label?: string;
  /** Index of the transition in the source task's `next` array */
  transitionIndex: number;
  /** Custom color override for the edge (CSS color string) */
  color?: string;
  /** Custom line style override for the edge */
  lineStyle?: LineStyle;
  /** Intermediate waypoints for this specific edge */
  waypoints?: NodePosition[];
  /** Label position as t-parameter (0–1) along the edge path; default 0.5 */
  labelPosition?: number;
}

/**
 * Cancellation policy for a workflow.
 *
 * Controls what happens to running tasks when a workflow is cancelled:
 * - `allow_finish` (default): Running tasks complete naturally; only
 *   pending/requested tasks are cancelled and no new tasks are dispatched.
 * - `cancel_running`: All running and pending tasks are forcefully cancelled.
 *   Running processes receive SIGINT → SIGTERM → SIGKILL via the worker.
 */
export type CancellationPolicy = "allow_finish" | "cancel_running";

/** Human-readable labels for each cancellation policy */
export const CANCELLATION_POLICY_LABELS: Record<CancellationPolicy, string> = {
  allow_finish: "Allow running tasks to finish",
  cancel_running: "Cancel running tasks",
};

/** Complete workflow builder state */
export interface WorkflowBuilderState {
  /** Workflow name (used to derive ref and filename) */
  name: string;
  /** Human-readable label */
  label: string;
  /** Description */
  description: string;
  /** Semantic version */
  version: string;
  /** Pack reference this workflow belongs to */
  packRef: string;
  /** Pack-level visibility for references to the workflow action */
  referenceVisibility: ReferenceVisibility;
  /** Pack refs allowed to reference the workflow action when visibility is restricted */
  referenceAllowedPackRefs: string[];
  /** Input parameter schema (flat format) */
  parameters: Record<string, ParamDefinition>;
  /** Output schema (flat format) */
  output: Record<string, ParamDefinition>;
  /**
   * Output mapping — keys are output field names, values are template
   * expressions evaluated against the WorkflowContext on completion. Maps
   * directly to YAML `output_map`.
   */
  outputMap: Record<string, string>;
  /** Workflow-scoped variables */
  vars: Record<string, unknown>;
  /** Task nodes */
  tasks: WorkflowTask[];
  /** Tags */
  tags: string[];
  /** Cancellation policy (default: allow_finish) */
  cancellationPolicy: CancellationPolicy;
}

/** Parameter definition in flat schema format */
export interface ParamDefinition {
  type: string;
  description?: string;
  required?: boolean;
  secret?: boolean;
  default?: unknown;
  enum?: string[];
  [key: string]: unknown;
}

/** Workflow definition as stored in the YAML file / API */
/**
 * Full workflow definition — used for DB storage and the save API payload.
 * Contains both action-level metadata AND the execution graph.
 */
export interface WorkflowYamlDefinition {
  ref: string;
  label: string;
  description?: string;
  version: string;
  parameters?: Record<string, unknown>;
  output?: Record<string, unknown>;
  vars?: Record<string, unknown>;
  tasks: WorkflowYamlTask[];
  output_map?: Record<string, string>;
  tags?: string[];
  cancellation_policy?: CancellationPolicy;
}

/**
 * Graph-only workflow definition — written to the `.workflow.yaml` file on disk.
 *
 * Action-linked workflow files contain only the execution graph. The companion
 * action YAML (`actions/{name}.yaml`) is authoritative for `ref`, `label`,
 * `description`, `parameters`, `output`, and `tags`.
 */
export interface WorkflowGraphDefinition {
  version: string;
  vars?: Record<string, unknown>;
  tasks: WorkflowYamlTask[];
  output_map?: Record<string, string>;
  cancellation_policy?: CancellationPolicy;
}

/**
 * Action YAML definition — written to the companion `actions/{name}.yaml` file.
 *
 * Controls the action's identity and exposed interface. References the workflow
 * file via `workflow_file`.
 */
export interface ActionYamlDefinition {
  ref: string;
  label: string;
  description?: string;
  reference_visibility?: ReferenceVisibility;
  reference_allowed_pack_refs?: string[];
  workflow_file: string;
  parameters?: Record<string, unknown>;
  output?: Record<string, unknown>;
  tags?: string[];
}

/** Chart-only metadata for a transition edge (not consumed by the backend) */
export interface TransitionChartMeta {
  /** Custom display label for the transition */
  label?: string;
  /** Custom color for the transition edge (CSS color string) */
  color?: string;
  /** Custom line style for the transition edge */
  line_style?: LineStyle;
  /** Intermediate waypoints per target task (keyed by target task name) */
  edge_waypoints?: Record<string, NodePosition[]>;
  /** Label position per target task as t-parameter (0–1) along the edge path */
  label_positions?: Record<string, number>;
}

/** Transition as represented in YAML format */
export interface WorkflowYamlTransition {
  when?: string;
  publish?: PublishDirective[];
  do?: string[];
  /** Visual metadata (label, color, line style, waypoints) — ignored by backend */
  __chart_meta__?: TransitionChartMeta;
}

/** Chart-only metadata for a task node (not consumed by the backend) */
export interface TaskChartMeta {
  /** Visual position on the canvas */
  position?: NodePosition;
}

/** Task as represented in YAML format */
export interface WorkflowYamlTask {
  name: string;
  action?: string;
  input?: Record<string, unknown>;
  permission_set_refs?: string | string[];
  delay?: number;
  with_items?: string;
  iterate_cache?: WorkflowYamlIterateCacheConfig;
  batch_size?: number;
  concurrency?: number;
  retry?: RetryConfig;
  timeout?: number | string;
  next?: WorkflowYamlTransition[];
  join?: number;
  /** Visual metadata (position) — ignored by backend */
  __chart_meta__?: TaskChartMeta;
}

/** Request to save a workflow file to disk and sync to DB */
export interface SaveWorkflowFileRequest {
  /** Workflow name (becomes filename: {name}.workflow.yaml) */
  name: string;
  /** Human-readable label */
  label: string;
  /** Description */
  description?: string;
  /** Semantic version */
  version: string;
  /** Pack reference */
  pack_ref: string;
  /** Pack-level visibility for references to the companion workflow action */
  reference_visibility?: ReferenceVisibility;
  /** Pack refs allowed to reference the companion workflow action when visibility is restricted */
  reference_allowed_pack_refs?: string[];
  /** The full workflow definition as JSON */
  definition: WorkflowYamlDefinition;
  /** Parameter schema (flat format) */
  param_schema?: Record<string, unknown>;
  /** Output schema (flat format) */
  out_schema?: Record<string, unknown>;
  /** Tags */
  tags?: string[];
}

export const LOCAL_REF_PATTERN = /^[a-z][a-z0-9_-]*$/;
export const COMPONENT_REF_PATTERN = /^[a-z][a-z0-9_-]*\.[a-z][a-z0-9_-]*$/;
const IDENTIFIER_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;
const VALID_SCHEMA_TYPES = new Set([
  "string",
  "number",
  "integer",
  "boolean",
  "object",
  "array",
  "any",
]);

export function isValidLocalRef(ref: string): boolean {
  return LOCAL_REF_PATTERN.test(ref);
}

export function isValidComponentRef(ref: string): boolean {
  return COMPONENT_REF_PATTERN.test(ref);
}

/** An action summary used in the action palette */
export interface PaletteAction {
  id: number;
  ref: string;
  label: string;
  description: string;
  pack_ref: string;
}

export type ReferenceVisibility = "public" | "private" | "restricted";

// ---------------------------------------------------------------------------
// Conversion functions
// ---------------------------------------------------------------------------

/**
 * Check if two values are deeply equal for the purpose of default comparison.
 * Handles primitives, arrays, and plain objects.
 */
function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a == null || b == null) return false;
  if (typeof a !== typeof b) return false;
  if (typeof a !== "object") return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return false;
    return a.every((v, i) => deepEqual(v, b[i]));
  }
  const aObj = a as Record<string, unknown>;
  const bObj = b as Record<string, unknown>;
  const aKeys = Object.keys(aObj);
  const bKeys = Object.keys(bObj);
  if (aKeys.length !== bKeys.length) return false;
  return aKeys.every((key) => deepEqual(aObj[key], bObj[key]));
}

/**
 * Strip input values that match their schema defaults.
 * Returns a new object containing only user-modified values.
 */
export function stripDefaultInputs(
  input: Record<string, unknown>,
  paramSchema: Record<string, unknown> | null | undefined,
): Record<string, unknown> {
  if (!paramSchema || typeof paramSchema !== "object") return input;
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(input)) {
    const schemaDef = paramSchema[key] as
      { default?: unknown } | null | undefined;
    if (
      schemaDef &&
      schemaDef.default !== undefined &&
      deepEqual(value, schemaDef.default)
    ) {
      continue; // skip — matches default
    }
    // Also skip empty strings when there's no default (user never filled it in)
    if (value === "" && (!schemaDef || schemaDef.default === undefined)) {
      continue;
    }
    result[key] = value;
  }
  return result;
}

/**
 * Convert builder state to YAML definition for saving.
 *
 * When `actionSchemas` is provided (a map of action ref → param_schema),
 * input values that match their schema defaults are omitted from the output
 * so only user-modified parameters appear in the generated YAML.
 */
export function builderStateToDefinition(
  state: WorkflowBuilderState,
  actionSchemas?: Map<string, Record<string, unknown> | null>,
): WorkflowYamlDefinition {
  const graph = builderStateToGraph(state, actionSchemas);
  const definition: WorkflowYamlDefinition = {
    ref: `${state.packRef}.${state.name}`,
    label: state.label,
    version: state.version,
    tasks: graph.tasks,
  };

  if (state.description) {
    definition.description = state.description;
  }

  if (Object.keys(state.parameters).length > 0) {
    definition.parameters = state.parameters;
  }

  if (Object.keys(state.output).length > 0) {
    definition.output = state.output;
  }

  if (graph.vars && Object.keys(graph.vars).length > 0) {
    definition.vars = graph.vars;
  }

  if (graph.output_map) {
    definition.output_map = graph.output_map;
  }

  if (state.tags.length > 0) {
    definition.tags = state.tags;
  }

  if (state.cancellationPolicy !== "allow_finish") {
    definition.cancellation_policy = state.cancellationPolicy;
  }

  return definition;
}

/**
 * Extract the graph-only workflow definition from builder state.
 *
 * This produces the content that should be written to the `.workflow.yaml`
 * file on disk — no `ref`, `label`, `description`, `parameters`, `output`,
 * or `tags`. Those belong in the companion action YAML.
 */
export function builderStateToGraph(
  state: WorkflowBuilderState,
  actionSchemas?: Map<string, Record<string, unknown> | null>,
): WorkflowGraphDefinition {
  const tasks: WorkflowYamlTask[] = state.tasks.map((task) => {
    const yamlTask: WorkflowYamlTask = {
      name: task.name,
    };

    if (task.action) {
      yamlTask.action = task.action;
    }

    // Filter input: strip values that match schema defaults
    const schema = actionSchemas?.get(task.action);
    const effectiveInput = schema
      ? stripDefaultInputs(task.input, schema)
      : task.input;
    if (Object.keys(effectiveInput).length > 0) {
      yamlTask.input = effectiveInput;
    }

    if (task.permission_set_refs !== undefined) {
      yamlTask.permission_set_refs = task.permission_set_refs;
    }

    if (task.delay) yamlTask.delay = task.delay;
    if (task.with_items) yamlTask.with_items = task.with_items;
    if (task.iterate_cache) yamlTask.iterate_cache = task.iterate_cache;
    if (task.batch_size) yamlTask.batch_size = task.batch_size;
    if (task.concurrency) yamlTask.concurrency = task.concurrency;
    if (task.retry) yamlTask.retry = task.retry;
    if (task.timeout) yamlTask.timeout = task.timeout;
    if (task.join) yamlTask.join = task.join;

    // Persist canvas position in __chart_meta__ so layout is restored on reload
    yamlTask.__chart_meta__ = {
      position: { x: task.position.x, y: task.position.y },
    };

    // Serialize transitions as `next` array
    if (task.next && task.next.length > 0) {
      yamlTask.next = task.next.map((t) => {
        const yt: WorkflowYamlTransition = {};
        if (t.when) yt.when = t.when;
        if (t.publish && t.publish.length > 0) yt.publish = t.publish;
        if (t.do && t.do.length > 0) yt.do = t.do;
        // Store label/color/line_style/waypoints in __chart_meta__
        const hasChartMeta =
          t.label ||
          t.color ||
          t.line_style ||
          t.edge_waypoints ||
          t.label_positions;
        if (hasChartMeta) {
          yt.__chart_meta__ = {};
          if (t.label) yt.__chart_meta__.label = t.label;
          if (t.color) yt.__chart_meta__.color = t.color;
          if (t.line_style) yt.__chart_meta__.line_style = t.line_style;
          if (t.edge_waypoints && Object.keys(t.edge_waypoints).length > 0) {
            yt.__chart_meta__.edge_waypoints = t.edge_waypoints;
          }
          if (t.label_positions && Object.keys(t.label_positions).length > 0) {
            yt.__chart_meta__.label_positions = t.label_positions;
          }
        }
        return yt;
      });
    }

    return yamlTask;
  });

  const graph: WorkflowGraphDefinition = {
    version: state.version,
    tasks,
  };

  if (Object.keys(state.vars).length > 0) {
    graph.vars = state.vars;
  }

  if (Object.keys(state.outputMap).length > 0) {
    graph.output_map = { ...state.outputMap };
  }

  if (state.cancellationPolicy !== "allow_finish") {
    graph.cancellation_policy = state.cancellationPolicy;
  }

  return graph;
}

/**
 * Extract the action YAML definition from builder state.
 *
 * This produces the content for the companion `actions/{name}.yaml` file
 * that owns action-level metadata and references the workflow file.
 */
export function builderStateToActionYaml(
  state: WorkflowBuilderState,
): ActionYamlDefinition {
  const action: ActionYamlDefinition = {
    ref: `${state.packRef}.${state.name}`,
    label: state.label,
    workflow_file: `workflows/${state.name}.workflow.yaml`,
  };

  if (state.referenceVisibility !== "public") {
    action.reference_visibility = state.referenceVisibility;
  }

  if (
    state.referenceVisibility === "restricted" &&
    state.referenceAllowedPackRefs.length > 0
  ) {
    action.reference_allowed_pack_refs = state.referenceAllowedPackRefs;
  }

  if (state.description) {
    action.description = state.description;
  }

  if (Object.keys(state.parameters).length > 0) {
    action.parameters = state.parameters;
  }

  if (Object.keys(state.output).length > 0) {
    action.output = state.output;
  }

  if (state.tags.length > 0) {
    action.tags = state.tags;
  }

  return action;
}

// ---------------------------------------------------------------------------
// Legacy format conversion helpers
// ---------------------------------------------------------------------------

/** Legacy task fields that may appear in older workflow definitions */
interface LegacyYamlTask extends WorkflowYamlTask {
  on_success?: string;
  on_failure?: string;
  on_complete?: string;
  on_timeout?: string;
  decision?: { when?: string; next: string; default?: boolean }[];
  publish?: PublishDirective[];
}

/**
 * Convert legacy on_success/on_failure/etc fields to `next` transitions.
 * This allows the builder to load workflows saved in the old format.
 */
function legacyTransitionsToNext(task: LegacyYamlTask): TaskTransition[] {
  const transitions: TaskTransition[] = [];

  if (task.on_success) {
    transitions.push({
      when: "{{ succeeded() }}",
      do: [task.on_success],
    });
  }

  if (task.on_failure) {
    transitions.push({
      when: "{{ failed() }}",
      do: [task.on_failure],
    });
  }

  if (task.on_complete) {
    // on_complete = unconditional (fires regardless of success/failure)
    transitions.push({
      do: [task.on_complete],
    });
  }

  if (task.on_timeout) {
    transitions.push({
      when: "{{ timed_out() }}",
      do: [task.on_timeout],
    });
  }

  // Convert legacy decision branches
  if (task.decision) {
    for (const branch of task.decision) {
      transitions.push({
        when: branch.when || undefined,
        do: [branch.next],
      });
    }
  }

  // If legacy task had publish but no transitions, create a publish-only transition
  if (task.publish && task.publish.length > 0 && transitions.length === 0) {
    transitions.push({
      when: "{{ succeeded() }}",
      publish: task.publish,
    });
  } else if (
    task.publish &&
    task.publish.length > 0 &&
    transitions.length > 0
  ) {
    // Attach publish to the first succeeded transition, or the first transition
    const succeededIdx = transitions.findIndex(
      (t) => t.when && t.when.toLowerCase().includes("succeeded()"),
    );
    const idx = succeededIdx >= 0 ? succeededIdx : 0;
    transitions[idx].publish = task.publish;
  }

  return transitions;
}

/**
 * Convert a YAML definition back to builder state (for editing existing workflows).
 * Supports both new `next` format and legacy `on_success`/`on_failure` format.
 */
// ---------------------------------------------------------------------------
// Auto-layout
// ---------------------------------------------------------------------------

/** Approximate node bounding box used for layout spacing. Mirrors TaskNode. */
const AUTO_LAYOUT_NODE_WIDTH = 240;
const AUTO_LAYOUT_NODE_HEIGHT = 140;
/** Vertical gap between adjacent layers. */
const AUTO_LAYOUT_LAYER_GAP_Y = 80;
/** Horizontal gap between sibling nodes within the same layer. */
const AUTO_LAYOUT_NODE_GAP_X = 60;
/** Top-left origin for the laid-out graph. */
const AUTO_LAYOUT_ORIGIN_X = 80;
const AUTO_LAYOUT_ORIGIN_Y = 80;

/**
 * Compute a connection-aware layered layout for a set of workflow tasks.
 *
 * Implements a Sugiyama-style algorithm:
 *   1. Build a directed graph from `task.next[*].do[*]` references.
 *   2. Assign each task to a layer via longest-path relaxation from sources.
 *      Cycles are tolerated by capping iterations and ignoring back edges
 *      during crossing reduction.
 *   3. Reorder nodes within each layer using the barycentric heuristic with
 *      alternating down/up sweeps to minimize edge crossings between layers.
 *   4. Translate (layer, slot) coordinates into pixel positions, centering
 *      each layer around the widest layer for a balanced look.
 *
 * Mutates each task's `position` field and returns the same array.
 */
export function autoLayoutTasks(tasks: WorkflowTask[]): WorkflowTask[] {
  if (tasks.length === 0) return tasks;

  const n = tasks.length;
  const indexByName = new Map<string, number>();
  tasks.forEach((t, i) => indexByName.set(t.name, i));

  // Adjacency lists (deduped).
  const succs: number[][] = Array.from({ length: n }, () => []);
  const preds: number[][] = Array.from({ length: n }, () => []);
  for (let i = 0; i < n; i++) {
    const seen = new Set<number>();
    for (const tr of tasks[i].next || []) {
      for (const targetName of tr.do || []) {
        const j = indexByName.get(targetName);
        if (j === undefined || j === i || seen.has(j)) continue;
        seen.add(j);
        succs[i].push(j);
        preds[j].push(i);
      }
    }
  }

  // Layer assignment via longest-path relaxation. Bounded iterations make
  // this safe for cyclic graphs (back edges simply stop contributing once
  // layers stabilize at the iteration cap).
  const layer = new Array<number>(n).fill(0);
  const maxRelaxIters = n + 1;
  for (let iter = 0; iter < maxRelaxIters; iter++) {
    let changed = false;
    for (let i = 0; i < n; i++) {
      for (const j of succs[i]) {
        if (layer[j] < layer[i] + 1) {
          layer[j] = layer[i] + 1;
          changed = true;
        }
      }
    }
    if (!changed) break;
  }

  const maxLayer = layer.reduce((m, v) => Math.max(m, v), 0);
  const layers: number[][] = Array.from({ length: maxLayer + 1 }, () => []);
  for (let i = 0; i < n; i++) layers[layer[i]].push(i);

  // Track each node's slot index within its layer.
  const slotOf = new Array<number>(n).fill(0);
  const refreshSlots = () => {
    for (const arr of layers) {
      arr.forEach((idx, slot) => (slotOf[idx] = slot));
    }
  };
  refreshSlots();

  // Barycentric crossing reduction with alternating down/up sweeps.
  const SWEEPS = 24;
  for (let s = 0; s < SWEEPS; s++) {
    // Down sweep: order each non-source layer by mean predecessor slot.
    for (let l = 1; l < layers.length; l++) {
      const arr = layers[l];
      const bary = (idx: number) => {
        const ps = preds[idx].filter((p) => layer[p] === l - 1);
        if (ps.length === 0) return slotOf[idx];
        let sum = 0;
        for (const p of ps) sum += slotOf[p];
        return sum / ps.length;
      };
      arr.sort((a, b) => {
        const diff = bary(a) - bary(b);
        if (diff !== 0) return diff;
        return slotOf[a] - slotOf[b];
      });
    }
    refreshSlots();

    // Up sweep: order each non-sink layer by mean successor slot.
    for (let l = layers.length - 2; l >= 0; l--) {
      const arr = layers[l];
      const bary = (idx: number) => {
        const ss = succs[idx].filter((q) => layer[q] === l + 1);
        if (ss.length === 0) return slotOf[idx];
        let sum = 0;
        for (const q of ss) sum += slotOf[q];
        return sum / ss.length;
      };
      arr.sort((a, b) => {
        const diff = bary(a) - bary(b);
        if (diff !== 0) return diff;
        return slotOf[a] - slotOf[b];
      });
    }
    refreshSlots();
  }

  // Assign pixel coordinates. Each layer is centered horizontally against
  // the widest layer so the graph reads as a balanced top-down hierarchy.
  const stepX = AUTO_LAYOUT_NODE_WIDTH + AUTO_LAYOUT_NODE_GAP_X;
  const stepY = AUTO_LAYOUT_NODE_HEIGHT + AUTO_LAYOUT_LAYER_GAP_Y;
  const widest = layers.reduce((m, l) => Math.max(m, l.length), 1);
  const totalWidth = (widest - 1) * stepX;

  for (let l = 0; l < layers.length; l++) {
    const arr = layers[l];
    const layerWidth = (arr.length - 1) * stepX;
    const xOffset = AUTO_LAYOUT_ORIGIN_X + (totalWidth - layerWidth) / 2;
    for (let slot = 0; slot < arr.length; slot++) {
      const idx = arr[slot];
      tasks[idx] = {
        ...tasks[idx],
        position: {
          x: xOffset + slot * stepX,
          y: AUTO_LAYOUT_ORIGIN_Y + l * stepY,
        },
      };
    }
  }

  return tasks;
}

export function definitionToBuilderState(
  definition: WorkflowYamlDefinition,
  packRef: string,
  name: string,
): WorkflowBuilderState {
  const rawTasks = definition.tasks || [];
  const missingPositionFlags: boolean[] = rawTasks.map(
    (t) => !(t as LegacyYamlTask).__chart_meta__?.position,
  );

  const tasks: WorkflowTask[] = rawTasks.map((rawTask, index) => {
    const task = rawTask as LegacyYamlTask;
    const normalizedRetry = task.retry
      ? {
          ...task.retry,
          max_delay: normalizeNullable(
            task.retry.max_delay as number | null | undefined,
          ),
        }
      : undefined;

    // Determine transitions: prefer `next` if present, otherwise convert legacy fields
    let next: TaskTransition[] | undefined;
    if (task.next && task.next.length > 0) {
      next = task.next.map((t) => ({
        when: t.when,
        publish: t.publish,
        do: t.do,
        label: t.__chart_meta__?.label,
        color: t.__chart_meta__?.color,
        line_style: t.__chart_meta__?.line_style,
        edge_waypoints: t.__chart_meta__?.edge_waypoints,
        label_positions: t.__chart_meta__?.label_positions,
      }));
    } else {
      const converted = legacyTransitionsToNext(task);
      next = converted.length > 0 ? converted : undefined;
    }

    return {
      id: `task-${index}-${Date.now()}`,
      name: task.name,
      action: task.action || "",
      input: task.input || {},
      permission_set_refs: normalizeNullable(
        task.permission_set_refs as
          WorkflowTask["permission_set_refs"] | null | undefined,
      ),
      next,
      delay: normalizeNullable(task.delay as number | null | undefined),
      retry: normalizedRetry,
      timeout: normalizeNullable(task.timeout as number | string | null | undefined),
      with_items: normalizeNullable(
        task.with_items as string | null | undefined,
      ),
      iterate_cache: task.iterate_cache
        ? {
            owner_type: task.iterate_cache.owner_type,
            owner_ref: normalizeNullable(task.iterate_cache.owner_ref),
            namespace: task.iterate_cache.namespace,
            generation: task.iterate_cache.generation || "active",
            page_size: task.iterate_cache.page_size ?? 100,
            require_fresh: task.iterate_cache.require_fresh ?? false,
          }
        : undefined,
      batch_size: normalizeNullable(
        task.batch_size as number | null | undefined,
      ),
      concurrency: normalizeNullable(
        task.concurrency as number | null | undefined,
      ),
      join: normalizeNullable(task.join as number | null | undefined),
      // Placeholder; overwritten below if the workflow needs auto-layout.
      position: task.__chart_meta__?.position ?? {
        x: 300,
        y: 80 + index * 160,
      },
    };
  });

  // If any task lacks persisted position metadata, run a connection-aware
  // auto-layout for the whole graph so the chart presents cleanly with
  // minimal edge crossings. Saved positions are preserved when every task
  // has them.
  if (missingPositionFlags.some(Boolean) && tasks.length > 0) {
    autoLayoutTasks(tasks);
  }

  return {
    name,
    label: definition.label,
    description: definition.description || "",
    version: definition.version,
    packRef,
    referenceVisibility: "public",
    referenceAllowedPackRefs: [],
    parameters: (definition.parameters || {}) as Record<
      string,
      ParamDefinition
    >,
    output: (definition.output || {}) as Record<string, ParamDefinition>,
    outputMap: (definition.output_map || {}) as Record<string, string>,
    vars: definition.vars || {},
    tasks,
    tags: definition.tags || [],
    cancellationPolicy: definition.cancellation_policy || "allow_finish",
  };
}

// ---------------------------------------------------------------------------
// Edge derivation
// ---------------------------------------------------------------------------

/**
 * Derive visual edges from task transitions.
 *
 * Each entry in a task's `next` array can target multiple tasks via `do`.
 * Each target produces a separate edge with the same visual type/label.
 */
export function deriveEdges(tasks: WorkflowTask[]): WorkflowEdge[] {
  const edges: WorkflowEdge[] = [];
  const taskNameToId = new Map<string, string>();

  for (const task of tasks) {
    taskNameToId.set(task.name, task.id);
  }

  for (const task of tasks) {
    if (!task.next) continue;

    for (let ti = 0; ti < task.next.length; ti++) {
      const transition = task.next[ti];
      const edgeType = classifyTransitionWhen(transition.when);
      const label = transitionLabel(transition.when, transition.label);

      if (transition.do) {
        for (const targetName of transition.do) {
          const targetId = taskNameToId.get(targetName);
          if (targetId) {
            edges.push({
              from: task.id,
              to: targetId,
              toName: targetName,
              type: edgeType,
              label,
              transitionIndex: ti,
              color: transition.color,
              lineStyle: transition.line_style,
              waypoints: transition.edge_waypoints?.[targetName],
              labelPosition: transition.label_positions?.[targetName],
            });
          }
        }
      }
    }
  }

  return edges;
}

// ---------------------------------------------------------------------------
// Task transition helpers
// ---------------------------------------------------------------------------

/**
 * Find or create a transition in a task's `next` array that matches a preset.
 *
 * If a transition with a matching `when` expression already exists, returns
 * its index. Otherwise, appends a new transition and returns the new index.
 */
export function findOrCreateTransition(
  task: WorkflowTask,
  preset: TransitionPreset,
): { next: TaskTransition[]; index: number } {
  const whenExpr = PRESET_WHEN[preset];
  const next = [...(task.next || [])];

  // Look for an existing transition with the same `when`
  const existingIndex = next.findIndex((t) => {
    if (whenExpr === undefined) return t.when === undefined;
    return (
      t.when?.toLowerCase().replace(/\s+/g, "") ===
      whenExpr.toLowerCase().replace(/\s+/g, "")
    );
  });

  if (existingIndex >= 0) {
    return { next, index: existingIndex };
  }

  // Create new transition with default label, color, and line style for the preset
  const newTransition: TaskTransition = {
    label: PRESET_LABELS[preset],
    color: PRESET_COLORS[preset],
    line_style: PRESET_STYLES[preset],
  };
  if (whenExpr) newTransition.when = whenExpr;
  next.push(newTransition);
  return { next, index: next.length - 1 };
}

/**
 * Add a target task to a transition's `do` list.
 * If the target is already in the list, this is a no-op.
 * Returns the updated `next` array.
 */
export function addTransitionTarget(
  task: WorkflowTask,
  preset: TransitionPreset,
  targetTaskName: string,
): TaskTransition[] {
  const { next, index } = findOrCreateTransition(task, preset);
  const transition = { ...next[index] };
  const doList = [...(transition.do || [])];

  if (!doList.includes(targetTaskName)) {
    doList.push(targetTaskName);
  }

  transition.do = doList;
  next[index] = transition;
  return next;
}

/**
 * Remove all references to a task name from all transitions.
 * Cleans up transitions that become empty (no `do` and no `publish`).
 */
export function removeTaskFromTransitions(
  next: TaskTransition[] | undefined,
  taskName: string,
): TaskTransition[] | undefined {
  if (!next) return undefined;

  const cleaned = next
    .map((t) => {
      if (!t.do || !t.do.includes(taskName)) return t;
      const newDo = t.do.filter((name) => name !== taskName);
      // Also clean up waypoint/label entries for the removed target
      const updatedWaypoints = t.edge_waypoints
        ? Object.fromEntries(
            Object.entries(t.edge_waypoints).filter(([k]) => k !== taskName),
          )
        : undefined;
      const updatedLabelPos = t.label_positions
        ? Object.fromEntries(
            Object.entries(t.label_positions).filter(([k]) => k !== taskName),
          )
        : undefined;
      return {
        ...t,
        do: newDo.length > 0 ? newDo : undefined,
        edge_waypoints:
          updatedWaypoints && Object.keys(updatedWaypoints).length > 0
            ? updatedWaypoints
            : undefined,
        label_positions:
          updatedLabelPos && Object.keys(updatedLabelPos).length > 0
            ? updatedLabelPos
            : undefined,
      };
    })
    // Keep transitions that still have `do` targets or `publish` directives
    .filter(
      (t) => (t.do && t.do.length > 0) || (t.publish && t.publish.length > 0),
    );

  return cleaned.length > 0 ? cleaned : undefined;
}

/**
 * Rename a task in all transition `do` lists.
 * Returns a new array (or undefined) only when something changed;
 * otherwise returns the original reference so callers can cheaply
 * detect a no-op via `===`.
 */
export function renameTaskInTransitions(
  next: TaskTransition[] | undefined,
  oldName: string,
  newName: string,
): TaskTransition[] | undefined {
  if (!next) return undefined;

  let changed = false;
  const updated = next.map((t) => {
    const hasDo = t.do && t.do.includes(oldName);
    const hasWaypoint = t.edge_waypoints && oldName in t.edge_waypoints;
    const hasLabelPos = t.label_positions && oldName in t.label_positions;

    if (!hasDo && !hasWaypoint && !hasLabelPos) return t;
    changed = true;

    const result = { ...t };

    if (hasDo) {
      result.do = t.do!.map((name) => (name === oldName ? newName : name));
    }

    if (hasWaypoint && t.edge_waypoints) {
      const entries = Object.entries(t.edge_waypoints).map(([k, v]) => [
        k === oldName ? newName : k,
        v,
      ]);
      result.edge_waypoints = Object.fromEntries(entries);
    }

    if (hasLabelPos && t.label_positions) {
      const entries = Object.entries(t.label_positions).map(([k, v]) => [
        k === oldName ? newName : k,
        v,
      ]);
      result.label_positions = Object.fromEntries(entries);
    }

    return result;
  });

  return changed ? updated : next;
}

/**
 * Find "starting" tasks — those whose name does not appear in any
 * transition `do` list (i.e. no other task transitions into them).
 * Returns a Set of task IDs.
 */
export function findStartingTaskIds(tasks: WorkflowTask[]): Set<string> {
  // Collect every task name that is referenced as a transition target
  const targeted = new Set<string>();
  for (const task of tasks) {
    if (!task.next) continue;
    for (const t of task.next) {
      if (t.do) {
        for (const name of t.do) {
          targeted.add(name);
        }
      }
    }
  }

  const startIds = new Set<string>();
  for (const task of tasks) {
    if (!targeted.has(task.name)) {
      startIds.add(task.id);
    }
  }
  return startIds;
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/**
 * Generate a unique task ID
 */
export function generateTaskId(): string {
  return `task-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`;
}

/**
 * Create a new empty task
 */
export function createEmptyTask(
  name: string,
  position: NodePosition,
): WorkflowTask {
  return {
    id: generateTaskId(),
    name,
    action: "",
    input: {},
    position,
  };
}

/**
 * Generate a unique task name that doesn't conflict with existing tasks
 */
export function generateUniqueTaskName(
  existingTasks: WorkflowTask[],
  baseName: string = "task",
): string {
  const existingNames = new Set(existingTasks.map((t) => t.name));
  let counter = existingTasks.length + 1;
  let name = `${baseName}_${counter}`;
  while (existingNames.has(name)) {
    counter++;
    name = `${baseName}_${counter}`;
  }
  return name;
}

type ActionSchemaMap = Map<string, Record<string, unknown> | null>;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function isPositiveInteger(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) > 0;
}

function isNonNegativeInteger(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) >= 0;
}

function isTemplateValue(value: unknown): boolean {
  return typeof value === "string" && value.trim().startsWith("{{");
}

function normalizeNullable<T>(value: T | null | undefined): T | undefined {
  return value === null || value === undefined ? undefined : value;
}

function validateTemplateSyntax(
  value: string | undefined,
  label: string,
  errors: string[],
  options: { requirePureExpression?: boolean } = {},
) {
  const trimmed = value?.trim() ?? "";
  if (!trimmed) return;

  const pureExpression = trimmed.startsWith("{{") && trimmed.endsWith("}}");
  if (options.requirePureExpression && !pureExpression) {
    errors.push(
      `${label} must be a template expression like {{ parameters.items }}`,
    );
    return;
  }

  if (!trimmed.includes("{{") && !trimmed.includes("}}")) return;

  let cursor = 0;
  while (cursor < trimmed.length) {
    const open = trimmed.indexOf("{{", cursor);
    const close = trimmed.indexOf("}}", cursor);
    if (close !== -1 && (open === -1 || close < open)) {
      errors.push(`${label} has a closing }} without a matching opening {{`);
      return;
    }
    if (open === -1) return;
    const matchingClose = trimmed.indexOf("}}", open + 2);
    if (matchingClose === -1) {
      errors.push(
        `${label} has an opening {{ without matching closing delimiters`,
      );
      return;
    }
    if (trimmed.slice(open + 2, matchingClose).trim().length === 0) {
      errors.push(`${label} contains an empty template expression`);
      return;
    }
    cursor = matchingClose + 2;
  }
}

function validateFlatSchema(
  schema: Record<string, ParamDefinition>,
  label: string,
  errors: string[],
) {
  for (const [fieldName, definition] of Object.entries(schema)) {
    const path = `${label} field "${fieldName}"`;
    if (!fieldName.trim()) {
      errors.push(`${label} contains a field with an empty name`);
    }

    const type = typeof definition.type === "string" ? definition.type : "";
    if (!VALID_SCHEMA_TYPES.has(type)) {
      errors.push(`${path} has unsupported type "${type || "missing"}"`);
    }

    if (
      definition.minLength !== undefined &&
      !isNonNegativeInteger(definition.minLength)
    ) {
      errors.push(`${path} minLength must be a non-negative integer`);
    }
    if (
      definition.maxLength !== undefined &&
      !isNonNegativeInteger(definition.maxLength)
    ) {
      errors.push(`${path} maxLength must be a non-negative integer`);
    }
    if (
      typeof definition.minLength === "number" &&
      typeof definition.maxLength === "number" &&
      definition.minLength > definition.maxLength
    ) {
      errors.push(`${path} minLength must be less than or equal to maxLength`);
    }

    if (
      definition.minimum !== undefined &&
      typeof definition.minimum !== "number"
    ) {
      errors.push(`${path} minimum must be a number`);
    }
    if (
      definition.maximum !== undefined &&
      typeof definition.maximum !== "number"
    ) {
      errors.push(`${path} maximum must be a number`);
    }
    if (
      typeof definition.minimum === "number" &&
      typeof definition.maximum === "number" &&
      definition.minimum > definition.maximum
    ) {
      errors.push(`${path} minimum must be less than or equal to maximum`);
    }

    if (
      typeof definition.pattern === "string" &&
      definition.pattern.length > 0
    ) {
      try {
        new RegExp(definition.pattern);
      } catch (err) {
        errors.push(
          `${path} pattern is not a valid regular expression: ${
            err instanceof Error ? err.message : "invalid regex"
          }`,
        );
      }
    }

    if (
      definition.enum !== undefined &&
      (!Array.isArray(definition.enum) ||
        definition.enum.some((item) => typeof item !== "string"))
    ) {
      errors.push(`${path} enum must be a list of strings`);
    }
  }
}

function validatePermissionSetRefs(
  value: WorkflowTask["permission_set_refs"],
  label: string,
  errors: string[],
) {
  if (value === undefined || value === null) return;

  if (typeof value === "string") {
    validateTemplateSyntax(value, label, errors, {
      requirePureExpression: value.trim().startsWith("{{"),
    });
    if (value.trim().startsWith("{{")) return;
    if (value !== "standard" && !isValidComponentRef(value)) {
      errors.push(
        `${label} must be "standard", a pack.permission_set ref, or a template`,
      );
    }
    return;
  }

  if (!Array.isArray(value)) {
    errors.push(`${label} must be a string, string array, or template`);
    return;
  }

  const seen = new Set<string>();
  for (const ref of value) {
    if (seen.has(ref)) {
      errors.push(`${label} contains duplicate ref "${ref}"`);
    }
    seen.add(ref);
    if (ref !== "standard" && !isValidComponentRef(ref)) {
      errors.push(`${label} contains invalid permission set ref "${ref}"`);
    }
  }
}

function validateActionInputs(
  task: WorkflowTask,
  actionSchemas: ActionSchemaMap | undefined,
  errors: string[],
) {
  const schema = actionSchemas?.get(task.action);
  if (!schema) return;

  for (const [paramName, rawDefinition] of Object.entries(schema)) {
    if (!isPlainObject(rawDefinition)) continue;

    const definition = rawDefinition as ParamDefinition;
    const value = task.input?.[paramName];
    const label = `Task "${task.name}" input "${paramName}"`;
    const type =
      typeof definition.type === "string" ? definition.type : "string";

    if (
      definition.required &&
      (value === undefined || value === null || value === "")
    ) {
      errors.push(`${label} is required`);
      continue;
    }

    if (
      value === undefined ||
      value === null ||
      value === "" ||
      isTemplateValue(value)
    ) {
      if (typeof value === "string") {
        validateTemplateSyntax(value, label, errors, {
          requirePureExpression: value.trim().startsWith("{{"),
        });
      }
      continue;
    }

    if (definition.enum && !definition.enum.includes(String(value))) {
      errors.push(`${label} must be one of: ${definition.enum.join(", ")}`);
    }

    switch (type) {
      case "boolean":
        if (typeof value !== "boolean")
          errors.push(`${label} must be a boolean`);
        break;
      case "number":
        if (typeof value !== "number" || Number.isNaN(value)) {
          errors.push(`${label} must be a number`);
        }
        break;
      case "integer":
        if (!Number.isInteger(value))
          errors.push(`${label} must be an integer`);
        break;
      case "array":
        if (!Array.isArray(value)) errors.push(`${label} must be an array`);
        break;
      case "object":
        if (!isPlainObject(value)) errors.push(`${label} must be an object`);
        break;
      default:
        break;
    }

    if (typeof value === "number") {
      if (
        typeof definition.minimum === "number" &&
        value < definition.minimum
      ) {
        errors.push(`${label} must be >= ${definition.minimum}`);
      }
      if (
        typeof definition.maximum === "number" &&
        value > definition.maximum
      ) {
        errors.push(`${label} must be <= ${definition.maximum}`);
      }
    }

    if (typeof value === "string") {
      if (
        typeof definition.minLength === "number" &&
        value.length < definition.minLength
      ) {
        errors.push(
          `${label} must be at least ${definition.minLength} characters`,
        );
      }
      if (
        typeof definition.maxLength === "number" &&
        value.length > definition.maxLength
      ) {
        errors.push(
          `${label} must be at most ${definition.maxLength} characters`,
        );
      }
      if (
        typeof definition.pattern === "string" &&
        definition.pattern.length > 0
      ) {
        try {
          const regex = new RegExp(definition.pattern);
          if (!regex.test(value)) {
            errors.push(`${label} must match pattern ${definition.pattern}`);
          }
        } catch {
          // The schema itself is validated separately.
        }
      }
    }
  }
}

/**
 * Validate a workflow builder state and return any errors.
 */
export function validateWorkflow(
  state: WorkflowBuilderState,
  actionSchemas?: ActionSchemaMap,
): string[] {
  const errors: string[] = [];

  if (!state.name.trim()) {
    errors.push("Workflow name is required");
  } else if (!isValidLocalRef(state.name)) {
    errors.push(
      "Workflow name must start with a lowercase letter and use only lowercase letters, numbers, underscores, or hyphens",
    );
  }

  if (!state.label.trim()) {
    errors.push("Workflow label is required");
  } else if (state.label.length > 255) {
    errors.push("Workflow label must be 255 characters or fewer");
  }

  if (!state.version.trim()) {
    errors.push("Workflow version is required");
  } else if (state.version.length > 50) {
    errors.push("Workflow version must be 50 characters or fewer");
  }

  if (!state.packRef) {
    errors.push("Pack reference is required");
  } else if (!isValidLocalRef(state.packRef)) {
    errors.push("Pack reference is not a valid pack ref");
  }

  if (
    state.referenceVisibility !== "restricted" &&
    state.referenceAllowedPackRefs.length > 0
  ) {
    errors.push(
      "Allowed pack refs can only be set when reference visibility is restricted",
    );
  }

  validateFlatSchema(state.parameters, "Workflow input schema", errors);
  validateFlatSchema(state.output, "Workflow output schema", errors);

  for (const [key, expression] of Object.entries(state.outputMap)) {
    if (!key.trim()) {
      errors.push("Workflow output map contains an empty output name");
    }
    validateTemplateSyntax(
      expression,
      `Workflow output "${key}" expression`,
      errors,
    );
  }

  if (state.tasks.length === 0) {
    errors.push("Workflow must have at least one task");
  }

  // Check for duplicate task names
  const taskNames = new Set<string>();
  const inboundCounts = new Map<string, number>();
  for (const task of state.tasks) {
    if (!task.name.trim()) {
      errors.push("Task name is required");
    } else if (!isValidLocalRef(task.name)) {
      errors.push(
        `Task "${task.name}" must start with a lowercase letter and use only lowercase letters, numbers, underscores, or hyphens`,
      );
    }
    if (taskNames.has(task.name)) {
      errors.push(`Duplicate task name: "${task.name}"`);
    }
    taskNames.add(task.name);
  }

  // Check that tasks have an action reference
  for (const task of state.tasks) {
    if (!task.action) {
      errors.push(`Task "${task.name}" must have an action assigned`);
    } else if (!isValidComponentRef(task.action)) {
      errors.push(`Task "${task.name}" action ref "${task.action}" is invalid`);
    }

    validatePermissionSetRefs(
      task.permission_set_refs,
      `Task "${task.name}" permission set refs`,
      errors,
    );
    validateActionInputs(task, actionSchemas, errors);

    if (
      task.delay !== undefined &&
      task.delay !== null &&
      !isPositiveInteger(task.delay)
    ) {
      errors.push(`Task "${task.name}" delay must be a positive integer`);
    }
    if (
      task.timeout !== undefined &&
      task.timeout !== null
    ) {
      if (typeof task.timeout === "number") {
        if (!isPositiveInteger(task.timeout)) {
          errors.push(`Task "${task.name}" timeout must be a positive integer`);
        }
      } else {
        validateTemplateSyntax(
          task.timeout,
          `Task "${task.name}" timeout`,
          errors,
          { requirePureExpression: true },
        );
      }
    }

    if (task.with_items !== undefined && task.with_items !== null) {
      validateTemplateSyntax(
        task.with_items,
        `Task "${task.name}" with_items`,
        errors,
        {
          requirePureExpression: true,
        },
      );
    }
    if (task.with_items?.trim() && task.iterate_cache) {
      errors.push(
        `Task "${task.name}" cannot define both with_items and iterate_cache`,
      );
    }
    if (task.iterate_cache) {
      const iterateCache = task.iterate_cache;
      if (!iterateCache.owner_type) {
        errors.push(`Task "${task.name}" cache owner type is required`);
      }
      if (
        ["pack", "action", "sensor"].includes(iterateCache.owner_type) &&
        !iterateCache.owner_ref?.trim()
      ) {
        errors.push(
          `Task "${task.name}" cache owner reference is required for ${iterateCache.owner_type} ownership`,
        );
      }
      validateTemplateSyntax(
        iterateCache.owner_ref,
        `Task "${task.name}" cache owner reference`,
        errors,
      );
      if (!iterateCache.namespace.trim()) {
        errors.push(`Task "${task.name}" cache namespace is required`);
      } else {
        validateTemplateSyntax(
          iterateCache.namespace,
          `Task "${task.name}" cache namespace`,
          errors,
        );
      }
      const generation = iterateCache.generation.trim();
      if (!generation) {
        errors.push(`Task "${task.name}" cache generation is required`);
      } else if (generation.startsWith("{{")) {
        validateTemplateSyntax(
          generation,
          `Task "${task.name}" cache generation`,
          errors,
          { requirePureExpression: true },
        );
      } else if (generation !== "active" && !/^[1-9][0-9]*$/.test(generation)) {
        errors.push(
          `Task "${task.name}" cache generation must be active, a positive generation ID, or an expression`,
        );
      }
      if (
        !Number.isInteger(iterateCache.page_size) ||
        iterateCache.page_size < 1 ||
        iterateCache.page_size > 1000
      ) {
        errors.push(
          `Task "${task.name}" cache page size must be between 1 and 1000`,
        );
      }
    }
    if (task.batch_size !== undefined && task.batch_size !== null) {
      if (!isPositiveInteger(task.batch_size)) {
        errors.push(
          `Task "${task.name}" batch size must be a positive integer`,
        );
      }
      if (!task.with_items?.trim() && !task.iterate_cache) {
        errors.push(
          `Task "${task.name}" batch size requires with_items or iterate_cache`,
        );
      }
    }
    if (task.concurrency !== undefined && task.concurrency !== null) {
      if (!isPositiveInteger(task.concurrency)) {
        errors.push(
          `Task "${task.name}" concurrency must be a positive integer`,
        );
      }
      if (!task.with_items?.trim() && !task.iterate_cache) {
        errors.push(
          `Task "${task.name}" concurrency requires with_items or iterate_cache`,
        );
      }
    }

    if (task.retry) {
      if (!isPositiveInteger(task.retry.count)) {
        errors.push(
          `Task "${task.name}" retry count must be a positive integer`,
        );
      }
      if (!isNonNegativeInteger(task.retry.delay)) {
        errors.push(
          `Task "${task.name}" retry delay must be a non-negative integer`,
        );
      }
      if (
        task.retry.max_delay !== undefined &&
        task.retry.max_delay !== null &&
        !isPositiveInteger(task.retry.max_delay)
      ) {
        errors.push(
          `Task "${task.name}" retry max delay must be a positive integer`,
        );
      }
      if (
        task.retry.max_delay !== undefined &&
        task.retry.max_delay !== null &&
        task.retry.max_delay < task.retry.delay
      ) {
        errors.push(
          `Task "${task.name}" retry max delay must be >= retry delay`,
        );
      }
    }
  }

  // Check that all transition targets reference existing tasks
  for (const task of state.tasks) {
    if (
      task.join !== undefined &&
      task.join !== null &&
      !isPositiveInteger(task.join)
    ) {
      errors.push(`Task "${task.name}" join count must be a positive integer`);
    }

    if (!task.next) continue;

    for (let ti = 0; ti < task.next.length; ti++) {
      const transition = task.next[ti];
      const transitionLabel = `Task "${task.name}" transition ${ti + 1}`;
      validateTemplateSyntax(
        transition.when,
        `${transitionLabel} condition`,
        errors,
      );

      if (
        (!transition.do || transition.do.length === 0) &&
        (!transition.publish || transition.publish.length === 0)
      ) {
        errors.push(`${transitionLabel} has no targets or published variables`);
      }

      const seenTargets = new Set<string>();
      for (const targetName of transition.do ?? []) {
        inboundCounts.set(targetName, (inboundCounts.get(targetName) ?? 0) + 1);
        if (seenTargets.has(targetName)) {
          errors.push(
            `${transitionLabel} targets "${targetName}" more than once`,
          );
        }
        seenTargets.add(targetName);

        if (!taskNames.has(targetName)) {
          const whenLabel = transition.when
            ? ` (when: ${transition.when})`
            : " (always)";
          errors.push(
            `Task "${task.name}" transition${whenLabel} references non-existent task "${targetName}"`,
          );
        }
      }

      const publishKeys = new Set<string>();
      for (const directive of transition.publish ?? []) {
        const entries = Object.entries(directive);
        if (entries.length === 0) {
          errors.push(`${transitionLabel} contains an empty publish directive`);
          continue;
        }
        for (const [key, value] of entries) {
          if (!key.trim()) {
            errors.push(
              `${transitionLabel} contains a publish variable with an empty name`,
            );
          } else if (!IDENTIFIER_PATTERN.test(key)) {
            errors.push(
              `${transitionLabel} publish variable "${key}" is not a valid identifier`,
            );
          }
          if (publishKeys.has(key)) {
            errors.push(`${transitionLabel} publishes "${key}" more than once`);
          }
          publishKeys.add(key);
          if (typeof value === "string") {
            validateTemplateSyntax(
              value,
              `${transitionLabel} publish "${key}"`,
              errors,
            );
          }
        }
      }
    }
  }

  for (const task of state.tasks) {
    if (task.join !== undefined && task.join !== null) {
      const inbound = inboundCounts.get(task.name) ?? 0;
      if (inbound === 0) {
        errors.push(
          `Task "${task.name}" join count is set but the task has no inbound transitions`,
        );
      } else if (task.join > inbound) {
        errors.push(
          `Task "${task.name}" join count (${task.join}) cannot exceed inbound transition count (${inbound})`,
        );
      }
    }
  }

  return errors;
}
