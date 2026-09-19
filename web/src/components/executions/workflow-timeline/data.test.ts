import { describe, expect, it } from "vitest";
import {
  WorkflowTaskWaitKind,
  WorkflowTaskWaitState,
  type ExecutionSummary,
} from "@/api";
import type { WorkflowTaskWait } from "@/hooks/useWorkflowTaskWaits";
import { buildEdges, buildSyntheticWaitTasks } from "./data";
import type { TimelineTask, WorkflowDefinition } from "./types";

const definition: WorkflowDefinition = {
  tasks: [
    {
      name: "request_approval",
      action: "slack.request_approval",
      next: [{ do: ["deploy"] }],
    },
    {
      name: "deploy",
      action: "core.deploy",
      wait_for: { inquiry: "{{ task.request_approval.inquiry_id }}" },
    },
  ],
};

const wait: WorkflowTaskWait = {
  id: 8,
  inquiry_id: 91,
  kind: WorkflowTaskWaitKind.INQUIRY,
  state: WorkflowTaskWaitState.WAITING,
  task_name: "deploy",
  created: "2026-08-05T10:00:00Z",
  updated: "2026-08-05T10:01:00Z",
};

const approvalExecution: ExecutionSummary = {
  id: 6,
  parent: 42,
  action_ref: "slack.request_approval",
  status: "completed",
  created: "2026-08-05T09:59:00Z",
  updated: "2026-08-05T10:00:00Z",
  workflow_task: { task_name: "request_approval" },
};

describe("workflow wait timeline transformation", () => {
  it("derives action, navigation, state, and graph edges for a synthetic wait", () => {
    const synthetic = buildSyntheticWaitTasks({
      waits: [wait],
      childExecutions: [approvalExecution],
      workflowDef: definition,
      parentExecutionId: 42,
      nowMs: Date.parse("2026-08-05T10:02:00Z"),
    });
    const executionTask = {
      id: "6",
      name: "request_approval",
      actionRef: "slack.request_approval",
      state: "completed",
      startMs: null,
      endMs: null,
      upstreamIds: [],
      downstreamIds: [],
      taskIndex: null,
      timedOut: false,
      retryCount: 0,
      maxRetries: 0,
      durationMs: null,
      destination: { kind: "execution", executionId: 6 },
    } satisfies TimelineTask;
    const tasks = [executionTask, ...synthetic];

    expect(synthetic[0]).toMatchObject({
      name: "deploy",
      actionRef: "core.deploy",
      state: "waiting",
      destination: { kind: "inquiry", inquiryId: 91 },
    });
    expect(buildEdges(tasks, [approvalExecution], definition)).toContainEqual(
      expect.objectContaining({ from: "6", to: "__wait_8__" }),
    );
  });

  it("suppresses only a wait with a matching direct child", () => {
    const nestedChild = {
      ...approvalExecution,
      id: 9,
      parent: 6,
      workflow_task: { task_name: "deploy" },
    };
    expect(
      buildSyntheticWaitTasks({
        waits: [wait],
        childExecutions: [nestedChild],
        workflowDef: definition,
        parentExecutionId: 42,
      }),
    ).toHaveLength(1);

    expect(
      buildSyntheticWaitTasks({
        waits: [wait],
        childExecutions: [{ ...nestedChild, parent: 42 }],
        workflowDef: definition,
        parentExecutionId: 42,
      }),
    ).toHaveLength(0);
  });
});
