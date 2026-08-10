import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import WorkflowDetailsPanel from "@/components/executions/WorkflowDetailsPanel";
import {
  useChildExecutions,
  useWorkflowCacheIterations,
} from "@/hooks/useExecutions";

vi.mock("@/hooks/useExecutions", () => ({
  useChildExecutions: vi.fn(),
  useWorkflowCacheIterations: vi.fn(),
}));

vi.mock("@/hooks/useExecutionStream", () => ({
  useExecutionStream: vi.fn(),
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
