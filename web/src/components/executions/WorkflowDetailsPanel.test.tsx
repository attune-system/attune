import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import WorkflowDetailsPanel from "@/components/executions/WorkflowDetailsPanel";
import { WorkflowTaskWaitKind, WorkflowTaskWaitState } from "@/api";
import {
  useChildExecutions,
  useWorkflowCacheIterations,
} from "@/hooks/useExecutions";
import { useWorkflow } from "@/hooks/useWorkflows";
import { useWorkflowTaskWaits } from "@/hooks/useWorkflowTaskWaits";

vi.mock("@/hooks/useExecutions", () => ({
  useChildExecutions: vi.fn(),
  useWorkflowCacheIterations: vi.fn(),
}));

vi.mock("@/hooks/useExecutionStream", () => ({
  useExecutionStream: vi.fn(),
}));

vi.mock("@/hooks/useWorkflows", () => ({
  useWorkflow: vi.fn(),
}));

vi.mock("@/hooks/useWorkflowTaskWaits", () => ({
  useWorkflowTaskWaits: vi.fn(),
}));

vi.mock("@/components/executions/workflow-timeline", () => ({
  default: () => <div>Timeline</div>,
}));

const parentExecution = {
  id: 42,
  action_ref: "example.cache_workflow",
  status: "running",
  created: "2026-08-05T10:00:00Z",
  updated: "2026-08-05T10:01:00Z",
};

function renderPanel() {
  return render(
    <MemoryRouter>
      <WorkflowDetailsPanel
        parentExecution={parentExecution}
        actionRef={parentExecution.action_ref}
        defaultTab="tasks"
      />
    </MemoryRouter>,
  );
}

describe("WorkflowDetailsPanel cache iteration status", () => {
  beforeEach(() => {
    vi.mocked(useChildExecutions).mockReturnValue({
      data: { items: [] },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useChildExecutions>);
    vi.mocked(useWorkflowTaskWaits).mockReturnValue({
      data: { data: [] },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowTaskWaits>);
    vi.mocked(useWorkflow).mockReturnValue({
      data: {
        data: {
          definition: {
            tasks: [{ name: "approve_deploy", action: "core.deploy" }],
          },
        },
      },
    } as ReturnType<typeof useWorkflow>);
  });

  it("renders the safe operational fields and bounds the error summary", () => {
    const longError = `bounded-${"x".repeat(700)}`;
    vi.mocked(useWorkflowCacheIterations).mockReturnValue({
      data: {
        data: [
          {
            task_name: "for_each_customer",
            namespace_id: 987654,
            generation_id: 73,
            state: "failed",
            scanned_count: 1200,
            dispatched_count: 1180,
            page_size: 250,
            batch_size: 50,
            concurrency: 4,
            created: "2026-08-05T10:00:00Z",
            updated: "2026-08-05T10:02:00Z",
            completed_at: "2026-08-05T10:02:00Z",
            error_summary: longError,
            last_external_id: "must-not-render",
            cursor: "must-not-render",
          },
        ],
      },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowCacheIterations>);

    renderPanel();

    expect(
      screen.getByRole("region", { name: "Workflow cache iterations" }),
    ).toBeInTheDocument();
    expect(screen.getByText("for_each_customer")).toBeInTheDocument();
    expect(screen.getByText("Generation #73")).toBeInTheDocument();
    expect(screen.getByText("Scanned 1,200")).toBeInTheDocument();
    expect(screen.getByText("Dispatched 1,180")).toBeInTheDocument();
    expect(
      screen.getByText(/Batch 50.*Page 250.*Concurrency 4/),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("time")).toHaveLength(3);
    expect(screen.queryByText("987654")).not.toBeInTheDocument();
    expect(screen.queryByText("must-not-render")).not.toBeInTheDocument();
    expect(screen.queryByText(longError)).not.toBeInTheDocument();
    expect(screen.getByText(/^bounded-x+…$/).textContent).toHaveLength(512);
  });

  it("shows a compact loading state", () => {
    vi.mocked(useWorkflowCacheIterations).mockReturnValue({
      data: undefined,
      isLoading: true,
      error: null,
    } as ReturnType<typeof useWorkflowCacheIterations>);

    renderPanel();

    expect(
      screen.getByText("Loading cache iteration status…"),
    ).toBeInTheDocument();
  });

  it("renders nothing when the endpoint is unsupported or returns no data", () => {
    vi.mocked(useWorkflowCacheIterations).mockReturnValue({
      data: { data: [], unsupported: true },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowCacheIterations>);

    const { container, rerender } = renderPanel();
    expect(container).toBeEmptyDOMElement();

    vi.mocked(useWorkflowCacheIterations).mockReturnValue({
      data: { data: [] },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowCacheIterations>);
    rerender(
      <MemoryRouter>
        <WorkflowDetailsPanel
          parentExecution={parentExecution}
          actionRef={parentExecution.action_ref}
        />
      </MemoryRouter>,
    );
    expect(container).toBeEmptyDOMElement();
  });
});

describe("WorkflowDetailsPanel waits", () => {
  beforeEach(() => {
    vi.mocked(useChildExecutions).mockReturnValue({
      data: { items: [] },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useChildExecutions>);
    vi.mocked(useWorkflowCacheIterations).mockReturnValue({
      data: { data: [], unsupported: true },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowCacheIterations>);
    vi.mocked(useWorkflow).mockReturnValue({
      data: {
        data: {
          definition: {
            tasks: [{ name: "approve_deploy", action: "core.deploy" }],
          },
        },
      },
    } as ReturnType<typeof useWorkflow>);
  });

  it("keeps a no-child waiting task visible, counted, and linked", () => {
    vi.mocked(useWorkflowTaskWaits).mockReturnValue({
      data: {
        data: [
          {
            id: 7,
            target_id: 91,
            kind: WorkflowTaskWaitKind.INQUIRY,
            state: WorkflowTaskWaitState.WAITING,
            task_name: "approve_deploy",
            created: "2026-08-05T10:00:00Z",
            updated: "2026-08-05T10:01:00Z",
          },
        ],
      },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowTaskWaits>);

    renderPanel();

    expect(screen.getByText("(1 task)")).toBeInTheDocument();
    expect(screen.getByText("approve_deploy")).toBeInTheDocument();
    expect(screen.getByText("core.deploy")).toBeInTheDocument();
    expect(screen.getByText("waiting").closest("a")).toHaveAttribute(
      "href",
      "/inquiries/91",
    );
  });

  it.each([
    ["execution", undefined, "/executions/91"],
    ["work_queue_item", "ops.deployments", "/queues/ops.deployments/items/91"],
  ] as const)("links a %s wait to its target", (kind, workQueueRef, href) => {
    vi.mocked(useWorkflowTaskWaits).mockReturnValue({
      data: {
        data: [
          {
            id: 7,
            target_id: 91,
            kind,
            state: WorkflowTaskWaitState.WAITING,
            task_name: "approve_deploy",
            work_queue_ref: workQueueRef,
            created: "2026-08-05T10:00:00Z",
            updated: "2026-08-05T10:01:00Z",
          },
        ],
      },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowTaskWaits>);

    renderPanel();
    expect(screen.getByText("waiting").closest("a")).toHaveAttribute(
      "href",
      href,
    );
  });

  it("disables a queue item wait without a queue ref", () => {
    vi.mocked(useWorkflowTaskWaits).mockReturnValue({
      data: {
        data: [
          {
            id: 7,
            target_id: 91,
            kind: "work_queue_item",
            state: WorkflowTaskWaitState.WAITING,
            task_name: "approve_deploy",
            created: "2026-08-05T10:00:00Z",
            updated: "2026-08-05T10:01:00Z",
          },
        ],
      },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowTaskWaits>);

    renderPanel();
    const row = screen.getByText("waiting").closest("[aria-disabled=true]");
    expect(row).toBeInTheDocument();
    expect(row?.closest("a")).toBeNull();
  });

  it.each([
    [WorkflowTaskWaitState.TIMED_OUT, "timeout"],
    [WorkflowTaskWaitState.CANCELLED, "cancelled"],
    [WorkflowTaskWaitState.FAILED, "failed"],
    [WorkflowTaskWaitState.RELEASED, "released (no child)"],
  ])("renders a %s wait without a child", (state, label) => {
    vi.mocked(useWorkflowTaskWaits).mockReturnValue({
      data: {
        data: [
          {
            id: 7,
            target_id: 91,
            kind: WorkflowTaskWaitKind.INQUIRY,
            state,
            task_name: "approve_deploy",
            created: "2026-08-05T10:00:00Z",
            updated: "2026-08-05T10:01:00Z",
            resolved_at: "2026-08-05T10:01:00Z",
          },
        ],
      },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowTaskWaits>);

    renderPanel();
    expect(screen.getByText(label)).toBeInTheDocument();
  });

  it("suppresses the wait once its direct child execution exists", () => {
    vi.mocked(useWorkflowTaskWaits).mockReturnValue({
      data: {
        data: [
          {
            id: 7,
            target_id: 91,
            kind: WorkflowTaskWaitKind.INQUIRY,
            state: WorkflowTaskWaitState.RELEASED,
            task_name: "approve_deploy",
            created: "2026-08-05T10:00:00Z",
            updated: "2026-08-05T10:01:00Z",
          },
        ],
      },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useWorkflowTaskWaits>);
    vi.mocked(useChildExecutions).mockReturnValue({
      data: {
        items: [
          {
            id: 101,
            parent: 42,
            action_ref: "core.deploy",
            status: "requested",
            created: "2026-08-05T10:01:00Z",
            updated: "2026-08-05T10:01:00Z",
            workflow_task: { task_name: "approve_deploy" },
          },
        ],
      },
      isLoading: false,
      error: null,
    } as ReturnType<typeof useChildExecutions>);

    renderPanel();
    expect(screen.queryByText("released (no child)")).not.toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: /approve_deploy/ }),
    ).toHaveAttribute("href", "/executions/101");
  });
});
