/**
 * Data transformation layer for the Workflow Timeline DAG.
 *
 * Converts raw execution summaries + workflow definition into the internal
 * timeline structures (TimelineTask, TimelineEdge, TimelineMilestone) that
 * the layout engine and renderer consume.
 */

import type { ExecutionSummary } from "@/api";
import { WorkflowTaskWaitState } from "@/api";
import type { WorkflowTaskWait } from "@/hooks/useWorkflowTaskWaits";
import type {
  TimelineTask,
  TimelineEdge,
  TimelineMilestone,
  TaskState,
  EdgeKind,
  WithItemsGroupInfo,
  WorkflowDefinition,
  WorkflowDefinitionTask,
  WorkflowDefinitionTransition,
} from "./types";
import { WITH_ITEMS_COLLAPSE_THRESHOLD } from "./types";

// ---------------------------------------------------------------------------
// Execution status → TaskState mapping
// ---------------------------------------------------------------------------

function toTaskState(status: string): TaskState {
  switch (status) {
    case "completed":
      return "completed";
    case "running":
      return "running";
    case "failed":
      return "failed";
    case "timeout":
      return "timeout";
    case "canceling":
    case "cancelled":
      return "cancelled";
    case "abandoned":
      return "abandoned";
    case "requested":
    case "scheduling":
    case "scheduled":
    case "pending":
    default:
      return "pending";
  }
}

// ---------------------------------------------------------------------------
// Classify a `when` expression into an edge kind
// ---------------------------------------------------------------------------

function classifyWhen(when?: string): EdgeKind {
  if (!when) return "always";
  const lower = when.toLowerCase().replace(/\s+/g, "");
  if (lower.includes("succeeded()")) return "success";
  if (lower.includes("failed()")) return "failure";
  if (lower.includes("timed_out()")) return "timeout";
  return "custom";
}

/**
 * Check whether a transition's `when` condition would have fired given the
 * source task's actual execution status.
 *
 * - `"always"` fires for any terminal status.
 * - `"success"` fires only when the source completed.
 * - `"failure"` fires only when the source failed (non-timeout).
 * - `"timeout"` fires only when the source timed out.
 * - `"custom"` is conservatively assumed to match (we can't evaluate the
 *    expression client-side).
 * - If the source task hasn't reached a terminal state yet, we allow
 *   all transitions so that in-progress workflows still show their
 *   potential edges.
 */
function transitionMatchesStatus(
  kind: EdgeKind,
  sourceStatus: string | undefined,
): boolean {
  // If the source hasn't executed or isn't terminal yet, show all edges
  // so the user can see the full graph for in-progress workflows.
  if (
    !sourceStatus ||
    !["completed", "failed", "timeout", "cancelled", "abandoned"].includes(
      sourceStatus,
    )
  ) {
    return true;
  }

  switch (kind) {
    case "always":
      return true;
    case "success":
      return sourceStatus === "completed";
    case "failure":
      return sourceStatus === "failed";
    case "timeout":
      return sourceStatus === "timeout";
    case "custom":
      // Can't evaluate custom expressions client-side — show them
      return true;
    default:
      return true;
  }
}

/** Derive a short human-readable label for a `when` expression */
function labelForWhen(when?: string, chartLabel?: string): string | undefined {
  if (chartLabel) return chartLabel;
  if (!when) return undefined; // unconditional — no label needed
  const lower = when.toLowerCase().replace(/\s+/g, "");
  if (lower.includes("succeeded()")) return "succeeded";
  if (lower.includes("failed()")) return "failed";
  if (lower.includes("timed_out()")) return "timed out";
  // Custom expression — truncate for display
  if (when.length > 30) return when.slice(0, 27) + "…";
  return when;
}

// ---------------------------------------------------------------------------
// Normalize legacy transition formats into a unified `next` array
// ---------------------------------------------------------------------------

function normalizeLegacyTransitions(
  task: WorkflowDefinitionTask,
): WorkflowDefinitionTransition[] {
  if (task.next && task.next.length > 0) return task.next;

  const transitions: WorkflowDefinitionTransition[] = [];

  const toArray = (v: string | string[] | undefined): string[] => {
    if (!v) return [];
    return typeof v === "string" ? [v] : v;
  };

  const successTargets = toArray(task.on_success);
  if (successTargets.length > 0) {
    transitions.push({ when: "{{ succeeded() }}", do: successTargets });
  }

  const failureTargets = toArray(task.on_failure);
  if (failureTargets.length > 0) {
    transitions.push({ when: "{{ failed() }}", do: failureTargets });
  }

  const completeTargets = toArray(task.on_complete);
  if (completeTargets.length > 0) {
    transitions.push({ do: completeTargets }); // unconditional
  }

  const timeoutTargets = toArray(task.on_timeout);
  if (timeoutTargets.length > 0) {
    transitions.push({ when: "{{ timed_out() }}", do: timeoutTargets });
  }

  return transitions;
}

// ---------------------------------------------------------------------------
// Build task name → execution ID(s) mapping
// ---------------------------------------------------------------------------

/**
 * Group child executions by their workflow task_name.
 *
 * For with_items tasks there may be multiple executions per task name.
 * We pick a canonical "representative" execution for each task name for
 * the purpose of edge routing (the earliest-started or lowest-index one).
 */
function groupByTaskName(
  executions: ExecutionSummary[],
): Map<string, ExecutionSummary[]> {
  const groups = new Map<string, ExecutionSummary[]>();
  for (const exec of executions) {
    const taskName = exec.workflow_task?.task_name;
    if (!taskName) continue;
    const list = groups.get(taskName) ?? [];
    list.push(exec);
    groups.set(taskName, list);
  }
  // Sort each group by task_index, then by created timestamp
  for (const [, list] of groups) {
    list.sort((a, b) => {
      const ai = a.workflow_task?.task_index ?? 0;
      const bi = b.workflow_task?.task_index ?? 0;
      if (ai !== bi) return ai - bi;
      return new Date(a.created).getTime() - new Date(b.created).getTime();
    });
  }
  return groups;
}

function timelineTaskIdsByName(tasks: TimelineTask[]): Map<string, string[]> {
  const groups = new Map<string, string[]>();
  for (const task of tasks) {
    const ids = groups.get(task.name) ?? [];
    ids.push(task.id);
    groups.set(task.name, ids);
  }
  return groups;
}

// ---------------------------------------------------------------------------
// Public API: buildTimelineTasks
// ---------------------------------------------------------------------------

/**
 * Convert child execution summaries into TimelineTask structures.
 *
 * The `workflowDef` is optional — when provided it's used to derive
 * dependency edges via the transition model. When absent, dependencies
 * are inferred from timing (heuristic).
 */
export function buildTimelineTasks(
  childExecutions: ExecutionSummary[],
  workflowDef?: WorkflowDefinition | null,
  /** Current wall-clock time (epoch ms). When supplied, running tasks extend
   *  to this value instead of a one-shot `Date.now()`, allowing the caller to
   *  drive periodic updates via a ticking state variable. */
  nowMs?: number,
): TimelineTask[] {
  // workflowDef is accepted for API symmetry with buildEdges but not used
  // in task construction (edges are derived separately).
  void workflowDef;
  const now = nowMs ?? Date.now();
  const tasks: TimelineTask[] = [];

  for (const exec of childExecutions) {
    const wt = exec.workflow_task;
    const taskName = wt?.task_name ?? `Task #${exec.id}`;

    // Determine time bounds
    const startedAt = wt?.started_at ?? exec.started_at;
    const startMs = startedAt ? new Date(startedAt).getTime() : null;

    // For terminal states, use updated as end time; for running use now
    const isTerminal = [
      "completed",
      "failed",
      "timeout",
      "cancelled",
      "abandoned",
    ].includes(exec.status);
    let endMs: number | null = null;
    if (isTerminal) {
      const completedAt = wt?.completed_at;
      endMs = completedAt
        ? new Date(completedAt).getTime()
        : new Date(exec.updated).getTime();
    } else if (exec.status === "running" && startMs) {
      endMs = now;
    }

    // Duration
    let durationMs: number | null = wt?.duration_ms ?? null;
    if (durationMs == null && startMs != null && endMs != null) {
      durationMs = endMs - startMs;
    }

    tasks.push({
      id: String(exec.id),
      name: taskName,
      actionRef: exec.action_ref,
      state: toTaskState(exec.status),
      startMs,
      endMs,
      upstreamIds: [], // filled in by buildEdges
      downstreamIds: [], // filled in by buildEdges
      taskIndex: wt?.task_index ?? null,
      timedOut: wt?.timed_out ?? false,
      retryCount: wt?.retry_count ?? 0,
      maxRetries: wt?.max_retries ?? 0,
      durationMs,
      destination: { kind: "execution", executionId: exec.id },
    });
  }

  return tasks;
}

function waitStateToTaskState(state: WorkflowTaskWaitState): TaskState {
  switch (state) {
    case WorkflowTaskWaitState.WAITING:
      return "waiting";
    case WorkflowTaskWaitState.RELEASED:
      return "released";
    case WorkflowTaskWaitState.TIMED_OUT:
      return "timeout";
    case WorkflowTaskWaitState.CANCELLED:
      return "cancelled";
    case WorkflowTaskWaitState.FAILED:
      return "failed";
  }
}

export function buildSyntheticWaitTasks({
  waits,
  childExecutions,
  workflowDef,
  parentExecutionId,
  nowMs = Date.now(),
}: {
  waits: WorkflowTaskWait[];
  childExecutions: ExecutionSummary[];
  workflowDef?: WorkflowDefinition | null;
  parentExecutionId: number;
  nowMs?: number;
}): TimelineTask[] {
  const dispatchedTaskNames = new Set(
    childExecutions
      .filter((execution) => execution.parent === parentExecutionId)
      .map((execution) => execution.workflow_task?.task_name)
      .filter((taskName): taskName is string => taskName != null),
  );
  const definitionsByName = new Map(
    workflowDef?.tasks?.map((task) => [task.name, task]) ?? [],
  );

  return waits
    .filter((wait) => !dispatchedTaskNames.has(wait.task_name))
    .map((wait) => {
      const definition = definitionsByName.get(wait.task_name);
      const startMs = new Date(wait.created).getTime();
      const endMs =
        wait.state === WorkflowTaskWaitState.WAITING
          ? nowMs
          : new Date(wait.resolved_at ?? wait.updated).getTime();

      return {
        id: `__wait_${wait.id}__`,
        name: wait.task_name,
        actionRef: definition?.action ?? wait.task_name,
        state: waitStateToTaskState(wait.state),
        startMs,
        endMs,
        upstreamIds: [],
        downstreamIds: [],
        taskIndex: null,
        timedOut: wait.state === WorkflowTaskWaitState.TIMED_OUT,
        retryCount: 0,
        maxRetries: 0,
        durationMs: Math.max(0, endMs - startMs),
        destination: { kind: "inquiry", inquiryId: wait.inquiry_id },
      };
    });
}

// ---------------------------------------------------------------------------
// Public API: collapseWithItemsGroups
// ---------------------------------------------------------------------------

/**
 * Detect with_items task groups that exceed the collapse threshold and merge
 * each group into a single synthetic TimelineTask.  The synthetic node spans
 * the full time range of its members and carries a `groupInfo` descriptor so
 * the renderer can display "task ×N" with a progress summary.
 *
 * Returns a new task array (individual member tasks removed, group nodes
 * inserted) plus a mapping from each removed member ID to the group node ID
 * so that `buildEdges` can redirect connectivity.
 *
 * Must be called BEFORE `buildEdges` because edges reference task IDs.
 */
export function collapseWithItemsGroups(
  tasks: TimelineTask[],
  _childExecutions: ExecutionSummary[],
  workflowDef?: WorkflowDefinition | null,
): {
  tasks: TimelineTask[];
  /** Map from removed member task ID → group node ID */
  memberToGroup: Map<string, string>;
} {
  // 1. Identify with_items groups: tasks sharing the same name where at
  //    least one has a non-null taskIndex (indicating with_items expansion).
  const byName = new Map<string, TimelineTask[]>();
  for (const t of tasks) {
    const list = byName.get(t.name) ?? [];
    list.push(t);
    byName.set(t.name, list);
  }

  const memberToGroup = new Map<string, string>();
  const removedIds = new Set<string>();
  const groupNodes: TimelineTask[] = [];

  for (const [name, group] of byName) {
    // Only collapse if this looks like a with_items expansion
    const hasIndices = group.some((t) => t.taskIndex != null);
    if (!hasIndices || group.length < WITH_ITEMS_COLLAPSE_THRESHOLD) continue;

    // Look up concurrency from the workflow definition
    let concurrency = 0;
    if (workflowDef?.tasks) {
      const defTask = workflowDef.tasks.find((dt) => dt.name === name);
      if (defTask?.concurrency != null) {
        concurrency = defTask.concurrency;
      }
    }

    // Compute aggregate time bounds
    let minStart: number | null = null;
    let maxEnd: number | null = null;
    for (const t of group) {
      if (t.startMs != null) {
        if (minStart === null || t.startMs < minStart) minStart = t.startMs;
      }
      if (t.endMs != null) {
        if (maxEnd === null || t.endMs > maxEnd) maxEnd = t.endMs;
      }
    }

    // Compute per-state counts
    let completed = 0;
    let failed = 0;
    let running = 0;
    let pending = 0;
    let timedOutCount = 0;
    let cancelled = 0;
    let anyTimedOut = false;

    for (const t of group) {
      switch (t.state) {
        case "completed":
          completed++;
          break;
        case "failed":
          failed++;
          break;
        case "running":
          running++;
          break;
        case "pending":
          pending++;
          break;
        case "timeout":
          timedOutCount++;
          anyTimedOut = true;
          break;
        case "cancelled":
        case "abandoned":
          cancelled++;
          break;
      }
      if (t.timedOut) anyTimedOut = true;
    }

    // Determine aggregate state: running if any are running/pending,
    // failed if any failed (and none still running), else completed
    let aggregateState: TaskState;
    if (running > 0 || pending > 0) {
      aggregateState = "running";
    } else if (failed > 0 || timedOutCount > 0) {
      aggregateState = failed > 0 ? "failed" : "timeout";
    } else if (cancelled > 0 && completed === 0) {
      aggregateState = "cancelled";
    } else {
      aggregateState = "completed";
    }

    const memberIds = group.map((t) => t.id);

    // Use a synthetic ID that won't collide with real execution IDs
    const groupId = `__group_${name}__`;

    const durationMs =
      minStart != null && maxEnd != null ? maxEnd - minStart : null;

    // Build display name — progress counter (N/M) rendered by TimelineRenderer
    const displayName = name;

    const groupInfo: WithItemsGroupInfo = {
      totalItems: group.length,
      completed,
      failed,
      running,
      pending,
      timedOut: timedOutCount,
      cancelled,
      concurrency,
      memberIds,
    };

    // Use the first member's execution as the representative (for action ref, etc.)
    const representative = group[0];

    groupNodes.push({
      id: groupId,
      name: displayName,
      actionRef: representative.actionRef,
      state: aggregateState,
      startMs: minStart,
      endMs: maxEnd,
      upstreamIds: [],
      downstreamIds: [],
      taskIndex: null,
      timedOut: anyTimedOut,
      retryCount: 0,
      maxRetries: 0,
      durationMs,
      destination: representative.destination,
      groupInfo,
    });

    // Record mappings and mark members for removal
    for (const t of group) {
      memberToGroup.set(t.id, groupId);
      removedIds.add(t.id);
    }
  }

  // Build final task list: keep non-removed tasks, append group nodes
  const result = tasks.filter((t) => !removedIds.has(t.id));
  result.push(...groupNodes);

  return { tasks: result, memberToGroup };
}

// ---------------------------------------------------------------------------
// Public API: buildEdges
// ---------------------------------------------------------------------------

/**
 * Determine the edge kind for an actual transition based on the predecessor
 * task's terminal status.
 */
function edgeKindFromPredecessorStatus(status: string): EdgeKind {
  switch (status) {
    case "completed":
      return "success";
    case "failed":
      return "failure";
    case "timeout":
      return "timeout";
    default:
      return "always";
  }
}

/**
 * Look up transition metadata (label, color) from the workflow definition
 * for a specific source→target edge.  Returns `undefined` values when the
 * definition is unavailable or the transition cannot be matched.
 */
function lookupTransitionMeta(
  sourceTaskName: string,
  targetTaskName: string,
  firedKind: EdgeKind,
  workflowDef?: WorkflowDefinition | null,
): { label?: string; color?: string } {
  if (!workflowDef?.tasks) return {};

  const defTask = workflowDef.tasks.find((t) => t.name === sourceTaskName);
  if (!defTask) return {};

  const transitions = normalizeLegacyTransitions(defTask);

  // Try to find a transition that targets this task AND matches the kind
  for (const tr of transitions) {
    if (!tr.do?.includes(targetTaskName)) continue;
    const trKind = classifyWhen(tr.when);
    if (trKind === firedKind || trKind === "always") {
      return {
        label: labelForWhen(tr.when, tr.__chart_meta__?.label),
        color: tr.__chart_meta__?.color,
      };
    }
  }

  // Fallback 1: prefer custom/always transitions that target this task
  // (custom conditions may not classify neatly into success/failure/timeout).
  for (const tr of transitions) {
    if (!tr.do?.includes(targetTaskName)) continue;
    const trKind = classifyWhen(tr.when);
    if (trKind === "custom" || trKind === "always") {
      return {
        label: labelForWhen(tr.when, tr.__chart_meta__?.label),
        color: tr.__chart_meta__?.color,
      };
    }
  }

  // Fallback 2: there IS a transition targeting this task, but its
  // classified kind doesn't match the actual fired kind.  This means the
  // definition says e.g. failed()→X but the predecessor actually
  // succeeded (and the task was reached via a different path).  Return
  // the color from the definition for visual consistency, but omit the
  // label to avoid displaying a misleading condition like "init failed"
  // on an edge that actually represents a different transition path.
  for (const tr of transitions) {
    if (!tr.do?.includes(targetTaskName)) continue;
    return {
      label: undefined,
      color: tr.__chart_meta__?.color,
    };
  }

  return {};
}

/**
 * Build dependency edges between timeline tasks.
 *
 * **Primary strategy** (`triggered_by` metadata):
 * When child executions carry `workflow_task.triggered_by`, we draw only the
 * edges that actually fired during execution.  This produces an accurate
 * representation of the path taken through the workflow — unused transitions
 * (e.g. a `failed()` branch when the task succeeded) are omitted.
 *
 * **Fallback** (definition-based / timing heuristic):
 * For older executions that lack `triggered_by` data, or when no workflow
 * definition is available, we fall back to drawing all defined transitions
 * or inferring edges from timing.
 */
/**
 * Helper: record full upstream/downstream connectivity on task objects
 * independently of visual edges.  This ensures milestone detection (fork/
 * merge junctions) works correctly regardless of how many visual edges we
 * choose to render.
 */
function linkConnectivity(
  taskById: Map<string, TimelineTask>,
  sourceIds: string[],
  targetIds: string[],
): void {
  for (const sid of sourceIds) {
    const src = taskById.get(sid);
    if (src) {
      for (const tid of targetIds) {
        src.downstreamIds.push(tid);
      }
    }
  }
  for (const tid of targetIds) {
    const tgt = taskById.get(tid);
    if (tgt) {
      for (const sid of sourceIds) {
        tgt.upstreamIds.push(sid);
      }
    }
  }
}

/**
 * Helper: push a bounded number of visual edges for a source→target group.
 * For small groups (≤5 per side) we create the full cross-product.
 * For larger groups we create representative pairs (first, last, and a
 * few evenly-spaced samples) — the milestone system handles the visual
 * simplification of the fan-out/fan-in.
 */
const VISUAL_EDGE_THRESHOLD = 5;

function pushGroupEdges(
  edges: TimelineEdge[],
  sourceIds: string[],
  targetIds: string[],
  kind: EdgeKind,
  label: string | undefined,
  color: string | undefined,
): void {
  if (
    sourceIds.length <= VISUAL_EDGE_THRESHOLD &&
    targetIds.length <= VISUAL_EDGE_THRESHOLD
  ) {
    // Small group — full cross-product
    for (const sid of sourceIds) {
      for (const tid of targetIds) {
        edges.push({ from: sid, to: tid, kind, label, color });
      }
    }
  } else {
    // Large group — sample representative edges.
    // Pick up to VISUAL_EDGE_THRESHOLD evenly-spaced indices from each side.
    const sampleIndices = (len: number, max: number): number[] => {
      if (len <= max) return Array.from({ length: len }, (_, i) => i);
      const indices = new Set<number>();
      indices.add(0);
      indices.add(len - 1);
      for (let i = 1; indices.size < max && i < max - 1; i++) {
        indices.add(Math.round((i * (len - 1)) / (max - 1)));
      }
      return [...indices].sort((a, b) => a - b);
    };
    const srcSample = sampleIndices(sourceIds.length, VISUAL_EDGE_THRESHOLD);
    const tgtSample = sampleIndices(targetIds.length, VISUAL_EDGE_THRESHOLD);

    // Connect each sampled source to each sampled target
    let first = true;
    for (const si of srcSample) {
      for (const ti of tgtSample) {
        edges.push({
          from: sourceIds[si],
          to: targetIds[ti],
          kind,
          label: first ? label : undefined,
          color,
        });
        first = false;
      }
    }
  }
}

export function buildEdges(
  tasks: TimelineTask[],
  childExecutions: ExecutionSummary[],
  workflowDef?: WorkflowDefinition | null,
  /** Map from collapsed member ID → group node ID (from collapseWithItemsGroups) */
  memberToGroup?: Map<string, string>,
): TimelineEdge[] {
  const edges: TimelineEdge[] = [];
  // Pre-build task lookup for connectivity tracking
  const globalTaskById = new Map(tasks.map((t) => [t.id, t]));

  // Helper: resolve a task ID through the collapse mapping.
  // If the ID was collapsed into a group node, return the group ID instead.
  const resolve = memberToGroup?.size
    ? (id: string) => memberToGroup.get(id) ?? id
    : (id: string) => id;

  // Check whether triggered_by data is available.  If ANY non-entry-point
  // execution has a populated triggered_by we use the actual-path strategy.
  const hasTriggeredBy = childExecutions.some(
    (e) => e.workflow_task?.triggered_by != null,
  );

  if (hasTriggeredBy) {
    // ---------------------------------------------------------------
    // Actual-path strategy: draw only edges that fired
    // ---------------------------------------------------------------
    const groups = groupByTaskName(childExecutions);

    // Map task name → list of execution IDs (for with_items fan-out).
    // Resolve through collapse mapping and deduplicate so that collapsed
    // group members all map to the single group node ID.
    const taskIdsByName = new Map<string, string[]>();
    for (const [name, execs] of groups) {
      const ids = [...new Set(execs.map((e) => resolve(String(e.id))))];
      taskIdsByName.set(name, ids);
    }

    // Map execution ID → execution summary (for status lookup)
    const execById = new Map<string, ExecutionSummary>();
    for (const e of childExecutions) {
      execById.set(String(e.id), e);
    }

    // Collect unique (predecessorName → taskName) pairs with edge metadata
    const pairsSeen = new Set<string>();

    for (const exec of childExecutions) {
      const wt = exec.workflow_task;
      if (!wt?.triggered_by) continue;

      const pairKey = `${wt.triggered_by}→${wt.task_name}`;
      if (pairsSeen.has(pairKey)) continue;
      pairsSeen.add(pairKey);

      const predecessorName = wt.triggered_by;
      const predecessorIds = taskIdsByName.get(predecessorName) ?? [];
      const targetIds = taskIdsByName.get(wt.task_name) ?? [];
      if (predecessorIds.length === 0 || targetIds.length === 0) continue;

      // Determine edge kind from the predecessor group's terminal status.
      // Use the first terminal predecessor's status as representative.
      let firedKind: EdgeKind = "success";
      for (const pid of predecessorIds) {
        const predExec = execById.get(pid);
        if (predExec) {
          firedKind = edgeKindFromPredecessorStatus(predExec.status);
          if (["completed", "failed", "timeout"].includes(predExec.status)) {
            break;
          }
        }
      }

      // Look up visual metadata (label, color) from the workflow definition
      const meta = lookupTransitionMeta(
        predecessorName,
        wt.task_name,
        firedKind,
        workflowDef,
      );

      // Resolve IDs through collapse mapping for connectivity and edges
      const resolvedPredecessorIds = [...new Set(predecessorIds.map(resolve))];
      const resolvedTargetIds = [...new Set(targetIds.map(resolve))];

      // Filter out self-edges (collapsed group → itself)
      const filteredTargetIds = resolvedTargetIds.filter(
        (tid) => !resolvedPredecessorIds.includes(tid),
      );
      if (filteredTargetIds.length === 0) continue;

      // Always record full connectivity so upstreamIds/downstreamIds are
      // correct for milestone detection (fork/merge junctions), regardless
      // of group size.  Visual edges are capped separately.
      linkConnectivity(
        globalTaskById,
        resolvedPredecessorIds,
        filteredTargetIds,
      );
      pushGroupEdges(
        edges,
        resolvedPredecessorIds,
        filteredTargetIds,
        firedKind,
        meta.label,
        meta.color,
      );
    }

    // ---------------------------------------------------------------
    // Supplement with definition-based edges for join tasks.
    //
    // `triggered_by` only records the *last* predecessor that caused
    // the advance, so for join tasks (which wait for N predecessors)
    // only one inbound edge gets created above.  We consult the
    // workflow definition to find ALL tasks that transition into a
    // join target, then draw the missing edges so that all
    // predecessor branches correctly show connectivity.
    // ---------------------------------------------------------------
    if (workflowDef?.tasks && workflowDef.tasks.length > 0) {
      // Build a set of join task names (explicit `join` field) and also
      // detect implicit joins: tasks that are the target of transitions
      // from multiple different source tasks in the definition.
      // Map target task name → Set of source task names that have ANY
      // transition to it (used for join detection).
      const inboundSourcesByTarget = new Map<string, Set<string>>();
      // Map "source→target" → list of transition conditions (EdgeKind)
      // that route from source to target. Used to filter supplemental
      // edges: we only draw an edge if the source's actual status matches
      // at least one of the declared transition conditions.
      const transitionKindsByPair = new Map<string, EdgeKind[]>();
      for (const defTask of workflowDef.tasks) {
        const transitions = normalizeLegacyTransitions(defTask);
        for (const tr of transitions) {
          if (!tr.do) continue;
          const trKind = classifyWhen(tr.when);
          for (const targetName of tr.do) {
            let sources = inboundSourcesByTarget.get(targetName);
            if (!sources) {
              sources = new Set();
              inboundSourcesByTarget.set(targetName, sources);
            }
            sources.add(defTask.name);

            const pk = `${defTask.name}→${targetName}`;
            const kinds = transitionKindsByPair.get(pk) ?? [];
            kinds.push(trKind);
            transitionKindsByPair.set(pk, kinds);
          }
        }
      }

      // Identify join targets: tasks with `join` property OR with
      // inbound edges from more than one distinct source task.
      const joinTargets = new Set<string>();
      for (const defTask of workflowDef.tasks) {
        if (defTask.join != null && defTask.join > 0) {
          joinTargets.add(defTask.name);
        }
      }
      for (const [targetName, sources] of inboundSourcesByTarget) {
        if (sources.size > 1) {
          joinTargets.add(targetName);
        }
      }

      // For each join target that has executions, add missing edges
      // from all definition-declared source tasks.
      for (const joinName of joinTargets) {
        const targetIds = taskIdsByName.get(joinName);
        if (!targetIds || targetIds.length === 0) continue;

        const defSources = inboundSourcesByTarget.get(joinName);
        if (!defSources) continue;

        for (const sourceName of defSources) {
          // Skip if this pair was already covered by triggered_by
          const pairKey = `${sourceName}→${joinName}`;
          if (pairsSeen.has(pairKey)) continue;
          pairsSeen.add(pairKey);

          const sourceIds = taskIdsByName.get(sourceName);
          if (!sourceIds || sourceIds.length === 0) continue;

          // Determine the source task's actual terminal status
          let sourceStatus: string | undefined;
          for (const sid of sourceIds) {
            const srcExec = execById.get(sid);
            if (
              srcExec &&
              ["completed", "failed", "timeout"].includes(srcExec.status)
            ) {
              sourceStatus = srcExec.status;
              break;
            }
          }

          // Check whether any of the definition's transitions from this
          // source to this join target would actually fire given the
          // source's real status.  If none match, this source didn't
          // contribute to the join target — skip the edge entirely.
          const declaredKinds =
            transitionKindsByPair.get(`${sourceName}→${joinName}`) ?? [];
          if (sourceStatus) {
            const anyMatch = declaredKinds.some((k) =>
              transitionMatchesStatus(k, sourceStatus),
            );
            if (!anyMatch) continue;
          }

          // Determine edge kind from the source task's actual status
          const edgeKind: EdgeKind = sourceStatus
            ? edgeKindFromPredecessorStatus(sourceStatus)
            : "success";

          // Look up visual metadata from the definition
          const meta = lookupTransitionMeta(
            sourceName,
            joinName,
            edgeKind,
            workflowDef,
          );

          // Resolve IDs through collapse mapping
          const resolvedSourceIds = [...new Set(sourceIds.map(resolve))];
          const resolvedTargetIds = [...new Set(targetIds.map(resolve))];
          const filteredTargetIds = resolvedTargetIds.filter(
            (tid) => !resolvedSourceIds.includes(tid),
          );
          if (filteredTargetIds.length === 0) continue;

          // Full connectivity + bounded visual edges
          linkConnectivity(
            globalTaskById,
            resolvedSourceIds,
            filteredTargetIds,
          );
          pushGroupEdges(
            edges,
            resolvedSourceIds,
            filteredTargetIds,
            edgeKind,
            meta.label,
            meta.color,
          );
        }
      }
    }

    // A wait has no execution yet, so triggered_by cannot describe its edges.
    // Fill only the definition edges that touch a synthetic wait node.
    if (
      workflowDef?.tasks &&
      tasks.some((task) => task.destination.kind === "inquiry")
    ) {
      const allTaskIdsByName = timelineTaskIdsByName(tasks);
      const syntheticIds = new Set(
        tasks
          .filter((task) => task.destination.kind === "inquiry")
          .map((task) => task.id),
      );
      const taskById = new Map(tasks.map((task) => [task.id, task]));
      const edgeKeys = new Set(edges.map((edge) => `${edge.from}→${edge.to}`));

      for (const definitionTask of workflowDef.tasks) {
        const sourceIds = allTaskIdsByName.get(definitionTask.name) ?? [];
        for (const transition of normalizeLegacyTransitions(definitionTask)) {
          const kind = classifyWhen(transition.when);
          const sourceState = sourceIds
            .map((id) => taskById.get(id)?.state)
            .find((state) => state != null);
          if (!transitionMatchesStatus(kind, sourceState)) continue;

          for (const targetName of transition.do ?? []) {
            const targetIds = allTaskIdsByName.get(targetName) ?? [];
            if (
              !sourceIds.some((id) => syntheticIds.has(id)) &&
              !targetIds.some((id) => syntheticIds.has(id))
            ) {
              continue;
            }

            const meta = lookupTransitionMeta(
              definitionTask.name,
              targetName,
              kind,
              workflowDef,
            );
            for (const sourceId of sourceIds) {
              for (const targetId of targetIds) {
                const key = `${sourceId}→${targetId}`;
                if (sourceId === targetId || edgeKeys.has(key)) continue;
                edgeKeys.add(key);
                linkConnectivity(globalTaskById, [sourceId], [targetId]);
                edges.push({
                  from: sourceId,
                  to: targetId,
                  kind,
                  label: meta.label,
                  color: meta.color,
                });
              }
            }
          }
        }
      }
    }

    // Deduplicate connectivity (linkConnectivity may add duplicates)
    for (const task of tasks) {
      task.upstreamIds = [...new Set(task.upstreamIds)];
      task.downstreamIds = [...new Set(task.downstreamIds)];
    }
  } else if (workflowDef?.tasks && workflowDef.tasks.length > 0) {
    // ---------------------------------------------------------------
    // Fallback: definition-based edge derivation (status-filtered)
    // Used for older executions that lack triggered_by metadata.
    //
    // Unlike the naive "draw all transitions" approach, this filters
    // transitions based on the source task's actual execution status.
    // For example, if task "initialize" completed successfully, only
    // its `succeeded()` and `always` transitions are drawn — the
    // `failed()` branch is omitted because that path was never taken.
    // ---------------------------------------------------------------
    const taskIdsByName = timelineTaskIdsByName(tasks);
    // Build a map of task name → representative terminal status
    const taskStatusByName = new Map<string, string | undefined>();
    const taskById = new Map(tasks.map((task) => [task.id, task]));
    for (const [name, ids] of taskIdsByName) {
      taskStatusByName.set(
        name,
        ids.map((id) => taskById.get(id)?.state).find((state) => state != null),
      );
    }

    for (const defTask of workflowDef.tasks) {
      const sourceIds = taskIdsByName.get(defTask.name);
      if (!sourceIds || sourceIds.length === 0) continue;

      const sourceStatus = taskStatusByName.get(defTask.name);

      const transitions = normalizeLegacyTransitions(defTask);
      for (const transition of transitions) {
        if (!transition.do || transition.do.length === 0) continue;

        const kind = classifyWhen(transition.when);

        // Skip transitions whose condition doesn't match the source
        // task's actual terminal status.  This prevents phantom edges
        // like "init failed" from appearing when the task succeeded.
        if (!transitionMatchesStatus(kind, sourceStatus)) continue;

        const label = labelForWhen(
          transition.when,
          transition.__chart_meta__?.label,
        );
        const color = transition.__chart_meta__?.color;

        for (const targetName of transition.do) {
          const targetIds = taskIdsByName.get(targetName);
          if (!targetIds || targetIds.length === 0) continue;

          const resolvedSourceIds = [...new Set(sourceIds.map(resolve))];
          const resolvedTargetIds = [...new Set(targetIds.map(resolve))];
          const filteredTargetIds = resolvedTargetIds.filter(
            (tid) => !resolvedSourceIds.includes(tid),
          );
          if (filteredTargetIds.length === 0) continue;
          linkConnectivity(
            globalTaskById,
            resolvedSourceIds,
            filteredTargetIds,
          );
          pushGroupEdges(
            edges,
            resolvedSourceIds,
            filteredTargetIds,
            kind,
            label,
            color,
          );
        }
      }
    }

    // Deduplicate connectivity
    for (const task of tasks) {
      task.upstreamIds = [...new Set(task.upstreamIds)];
      task.downstreamIds = [...new Set(task.downstreamIds)];
    }
  } else {
    // Heuristic fallback: infer edges from timing
    inferEdgesFromTiming(tasks, edges);
  }

  return edges;
}

// ---------------------------------------------------------------------------
// Timing-based edge inference (fallback)
// ---------------------------------------------------------------------------

/**
 * When no workflow definition is available, infer likely dependencies
 * from task timing:
 *   - Tasks that start after another task ends are likely downstream.
 *   - Among candidates, pick the closest predecessor.
 *
 * This produces a reasonable approximation but won't capture all patterns.
 */
function inferEdgesFromTiming(tasks: TimelineTask[], edges: TimelineEdge[]) {
  // Sort by start time
  const sorted = [...tasks]
    .filter((t) => t.startMs != null)
    .sort((a, b) => a.startMs! - b.startMs!);

  for (const task of sorted) {
    if (task.startMs == null) continue;

    // Find the best predecessor: a task whose end time is closest to
    // (and before or at) this task's start time
    let bestPredecessor: TimelineTask | null = null;
    let bestGap = Infinity;

    for (const candidate of sorted) {
      if (candidate.id === task.id) continue;
      if (candidate.endMs == null) continue;
      if (candidate.endMs > task.startMs!) continue; // must finish before we start

      const gap = task.startMs! - candidate.endMs;
      if (gap < bestGap) {
        bestGap = gap;
        bestPredecessor = candidate;
      }
    }

    if (bestPredecessor) {
      edges.push({
        from: bestPredecessor.id,
        to: task.id,
        kind: "success",
        label: undefined,
      });
      task.upstreamIds.push(bestPredecessor.id);
      bestPredecessor.downstreamIds.push(task.id);
    }
  }
}

// ---------------------------------------------------------------------------
// Public API: buildMilestones
// ---------------------------------------------------------------------------

/**
 * Create synthetic start/end milestones, and optionally merge/fork junctions.
 *
 * Milestones anchor the visual start and end of the timeline and provide
 * clean attachment points for fan-out / fan-in edges.
 */
export function buildMilestones(
  tasks: TimelineTask[],
  parentExecution: ExecutionSummary,
): {
  milestones: TimelineMilestone[];
  milestoneEdges: TimelineEdge[];
  suppressedEdgeKeys: Set<string>;
} {
  const milestones: TimelineMilestone[] = [];
  const milestoneEdges: TimelineEdge[] = [];
  /** Direct task→task edge keys that are replaced by milestone-routed paths.
   *  `computeLayout` should exclude these from `taskEdges` to avoid duplicates. */
  const suppressedEdgeKeys = new Set<string>();

  // Compute time bounds
  const startTimes = tasks
    .filter((t) => t.startMs != null)
    .map((t) => t.startMs!);

  const parentStartMs = parentExecution.started_at
    ? new Date(parentExecution.started_at).getTime()
    : new Date(parentExecution.created).getTime();

  const minTimeMs =
    startTimes.length > 0
      ? Math.min(parentStartMs, ...startTimes)
      : parentStartMs;

  // Start milestone
  const startId = "__start__";
  milestones.push({
    id: startId,
    kind: "start",
    timeMs: minTimeMs,
    label: "Start",
  });

  // Identify root tasks (no upstream among task nodes)
  const rootTasks = tasks.filter((t) => t.upstreamIds.length === 0);

  // --- Fork junction detection ---
  const FORK_THRESHOLD = 3;
  if (rootTasks.length > FORK_THRESHOLD) {
    const earliestRootStart = Math.min(
      ...rootTasks.map((t) => t.startMs ?? minTimeMs),
    );
    const forkTime = minTimeMs + (earliestRootStart - minTimeMs) * 0.5;
    const forkId = "__fork_start__";
    milestones.push({
      id: forkId,
      kind: "fork",
      timeMs: forkTime,
      label: `fan-out ×${rootTasks.length}`,
    });

    milestoneEdges.push({
      from: startId,
      to: forkId,
      kind: "success",
    });
    // Cap visual edges from fork to root tasks (same as merge cap)
    const FORK_EDGE_CAP = 8;
    if (rootTasks.length <= FORK_EDGE_CAP) {
      for (const root of rootTasks) {
        milestoneEdges.push({
          from: forkId,
          to: root.id,
          kind: "success",
        });
      }
    } else {
      // Sample evenly-spaced root tasks for visual edges
      const indices = new Set<number>();
      indices.add(0);
      indices.add(rootTasks.length - 1);
      for (
        let i = 1;
        indices.size < FORK_EDGE_CAP && i < FORK_EDGE_CAP - 1;
        i++
      ) {
        indices.add(
          Math.round((i * (rootTasks.length - 1)) / (FORK_EDGE_CAP - 1)),
        );
      }
      for (const idx of indices) {
        milestoneEdges.push({
          from: forkId,
          to: rootTasks[idx].id,
          kind: "success",
        });
      }
    }
  } else {
    // Connect start directly to root tasks
    for (const root of rootTasks) {
      milestoneEdges.push({
        from: startId,
        to: root.id,
        kind: "success",
      });
    }
  }

  // --- Internal merge junctions ---
  // Detect tasks that share the exact same set of upstreamIds (> 2 upstream).
  // Group by the upstream signature, then insert merge milestones.
  //
  // For large with_items groups the upstream set can be very large (100s or
  // 1000s).  We use a stable hash of the sorted IDs as the map key instead
  // of the full comma-joined string to avoid huge string comparisons.
  const taskByIdLocal = new Map(tasks.map((t) => [t.id, t]));

  // Map: signature key → { upstreamIds, downstreamTasks }
  const upstreamSignatureGroups = new Map<
    string,
    { upstreamIds: string[]; tasks: TimelineTask[] }
  >();
  for (const task of tasks) {
    if (task.upstreamIds.length <= 2) continue;
    const sorted = [...task.upstreamIds].sort();
    // Use a short hash for the map key to avoid huge string keys.
    // For groups up to ~20, the full join is fine.  Beyond that, hash.
    // For small sets the full join is a perfect key.  For large sets we
    // sample enough points that accidental collision within a single
    // workflow is effectively impossible (count + 5 evenly-spaced IDs).
    const sig =
      sorted.length <= 20
        ? sorted.join(",")
        : [
            `n${sorted.length}`,
            sorted[0],
            sorted[Math.floor(sorted.length * 0.25)],
            sorted[Math.floor(sorted.length * 0.5)],
            sorted[Math.floor(sorted.length * 0.75)],
            sorted[sorted.length - 1],
          ].join("_");
    const group = upstreamSignatureGroups.get(sig);
    if (group) {
      group.tasks.push(task);
    } else {
      upstreamSignatureGroups.set(sig, {
        upstreamIds: sorted,
        tasks: [task],
      });
    }
  }

  // Maximum number of visual edges from upstream tasks to a merge milestone.
  // Beyond this we sample evenly-spaced representatives — the merge diamond
  // still conceptually represents the full fan-in.
  const MERGE_EDGE_CAP = 8;

  const sampleIds = (ids: string[], max: number): string[] => {
    if (ids.length <= max) return ids;
    const result = new Set<string>();
    result.add(ids[0]);
    result.add(ids[ids.length - 1]);
    for (let i = 1; result.size < max && i < max - 1; i++) {
      result.add(ids[Math.round((i * (ids.length - 1)) / (max - 1))]);
    }
    return [...result];
  };

  for (const [
    ,
    { upstreamIds, tasks: downstreamGroup },
  ] of upstreamSignatureGroups) {
    if (downstreamGroup.length < 1) continue;
    if (upstreamIds.length <= 2) continue;

    // Find the time position: latest end of the upstream tasks
    let maxEndMs = 0;
    for (const uid of upstreamIds) {
      const t = taskByIdLocal.get(uid);
      if (t) {
        const end = t.endMs ?? t.startMs ?? 0;
        if (end > maxEndMs) maxEndMs = end;
      }
    }
    const mergeTimeMs = maxEndMs > 0 ? maxEndMs + 50 : minTimeMs;

    const mergeId = `__merge_${upstreamIds.length}_${upstreamIds[0]}__`;

    // Avoid duplicating milestones
    if (milestones.some((m) => m.id === mergeId)) continue;

    // Determine label: if all upstream tasks share the same name, this is
    // a with_items convergence ("all items done"); otherwise it's a join
    // of distinct parallel branches ("join N").
    let allSameName = true;
    let firstName: string | null = null;
    for (const uid of upstreamIds) {
      const t = taskByIdLocal.get(uid);
      if (!t) continue;
      if (firstName === null) {
        firstName = t.name;
      } else if (t.name !== firstName) {
        allSameName = false;
        break;
      }
    }
    const mergeLabel =
      allSameName && firstName != null && upstreamIds.length > 1
        ? `all ${upstreamIds.length} items done`
        : `join ${upstreamIds.length}`;

    milestones.push({
      id: mergeId,
      kind: "merge",
      timeMs: mergeTimeMs,
      label: mergeLabel,
    });

    // Redirect edges: replace direct upstream→downstream edges with
    // upstream→merge→downstream.  For large fan-ins we only draw a
    // capped number of visual edges to the merge diamond to keep SVG
    // rendering fast; ALL direct edges are still suppressed so the
    // merge cleanly replaces them.
    const sampledUpstream = sampleIds(upstreamIds, MERGE_EDGE_CAP);
    for (const uid of sampledUpstream) {
      milestoneEdges.push({
        from: uid,
        to: mergeId,
        kind: "success",
      });
    }
    // Suppress ALL direct edges (not just the sampled ones)
    for (const uid of upstreamIds) {
      for (const dt of downstreamGroup) {
        suppressedEdgeKeys.add(`${uid}→${dt.id}`);
      }
    }
    for (const dt of downstreamGroup) {
      milestoneEdges.push({
        from: mergeId,
        to: dt.id,
        kind: "success",
      });
    }
  }

  return { milestones, milestoneEdges, suppressedEdgeKeys };
}

// ---------------------------------------------------------------------------
// Public API: findConnectedPath
// ---------------------------------------------------------------------------

/**
 * Given a selected task ID, find all upstream and downstream tasks
 * reachable within `maxHops` hops. Returns a Set of connected node IDs
 * (including the selected node itself) and a Set of connected edge keys.
 */
export function findConnectedPath(
  selectedId: string,
  tasks: TimelineTask[],
  allEdges: TimelineEdge[],
  maxHops: number = 999,
): { nodeIds: Set<string>; edgeKeys: Set<string> } {
  const nodeIds = new Set<string>();
  const edgeKeys = new Set<string>();
  const taskMap = new Map(tasks.map((t) => [t.id, t]));

  nodeIds.add(selectedId);

  // BFS upstream
  const upQueue: { id: string; depth: number }[] = [
    { id: selectedId, depth: 0 },
  ];
  while (upQueue.length > 0) {
    const { id, depth } = upQueue.shift()!;
    if (depth >= maxHops) continue;
    const task = taskMap.get(id);
    if (!task) continue;
    for (const uid of task.upstreamIds) {
      edgeKeys.add(edgeKey(uid, id));
      if (!nodeIds.has(uid)) {
        nodeIds.add(uid);
        upQueue.push({ id: uid, depth: depth + 1 });
      }
    }
  }

  // BFS downstream
  const downQueue: { id: string; depth: number }[] = [
    { id: selectedId, depth: 0 },
  ];
  while (downQueue.length > 0) {
    const { id, depth } = downQueue.shift()!;
    if (depth >= maxHops) continue;
    const task = taskMap.get(id);
    if (!task) continue;
    for (const did of task.downstreamIds) {
      edgeKeys.add(edgeKey(id, did));
      if (!nodeIds.has(did)) {
        nodeIds.add(did);
        downQueue.push({ id: did, depth: depth + 1 });
      }
    }
  }

  // Also include milestone edges that touch connected nodes
  for (const edge of allEdges) {
    if (nodeIds.has(edge.from) && nodeIds.has(edge.to)) {
      edgeKeys.add(edgeKey(edge.from, edge.to));
    }
  }

  return { nodeIds, edgeKeys };
}

/** Create a stable key for an edge (used for Set membership) */
export function edgeKey(from: string, to: string): string {
  return `${from}→${to}`;
}
