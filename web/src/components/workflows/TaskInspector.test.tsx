import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkflowTask } from "@/types/workflow";
import TaskInspector from "./TaskInspector";

const useAction = vi.fn();

vi.mock("@/hooks/useActions", () => ({
  useAction: (...args: unknown[]) => useAction(...args),
}));

function task(overrides: Partial<WorkflowTask> = {}): WorkflowTask {
  return {
    id: "task-1",
    name: "deploy",
    action: "core.echo",
    input: {},
    position: { x: 0, y: 0 },
    ...overrides,
  };
}

describe("TaskInspector wait authoring", () => {
  beforeEach(() => {
    useAction.mockReturnValue({ data: undefined, isLoading: false });
  });

  it("shows and edits a canonical inquiry wait", () => {
    const onUpdate = vi.fn();
    render(
      <TaskInspector
        task={task({
          wait_for: { inquiry: "{{ task.request_approval.inquiry_id }}" },
        })}
        allTaskNames={["deploy"]}
        availableActions={[]}
        onUpdate={onUpdate}
        onClose={vi.fn()}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /Wait for prerequisite/i }),
    );

    expect(screen.getByLabelText("Wait type")).toHaveValue("inquiry");
    expect(screen.getByLabelText("Inquiry ID or expression")).toHaveValue(
      "{{ task.request_approval.inquiry_id }}",
    );

    fireEvent.change(screen.getByLabelText("Wait type"), {
      target: { value: "execution" },
    });

    expect(onUpdate).toHaveBeenCalledWith("task-1", {
      wait_for: { execution: "" },
    });
  });

  it("disables wait authoring while iteration is configured", () => {
    render(
      <TaskInspector
        task={task({ with_items: "{{ parameters.hosts }}" })}
        allTaskNames={["deploy"]}
        availableActions={[]}
        onUpdate={vi.fn()}
        onClose={vi.fn()}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /Wait for prerequisite/i }),
    );

    expect(screen.getByLabelText("Wait type")).toBeDisabled();
    expect(
      screen.getByText(/Remove iteration before adding a prerequisite wait/i),
    ).toBeInTheDocument();
  });

  it("normalizes a padded literal wait target to a number", () => {
    const onUpdate = vi.fn();
    const props = {
      allTaskNames: ["deploy"],
      availableActions: [],
      onUpdate,
      onClose: vi.fn(),
    };
    const { rerender } = render(
      <TaskInspector task={task({ wait_for: { inquiry: "" } })} {...props} />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /Wait for prerequisite/i }),
    );
    const input = screen.getByLabelText("Inquiry ID or expression");
    fireEvent.change(input, { target: { value: " 42 " } });
    rerender(
      <TaskInspector
        task={task({ wait_for: { inquiry: " 42 " } })}
        {...props}
      />,
    );
    fireEvent.blur(screen.getByLabelText("Inquiry ID or expression"));

    expect(onUpdate).toHaveBeenLastCalledWith("task-1", {
      wait_for: { inquiry: 42 },
    });
  });
});
