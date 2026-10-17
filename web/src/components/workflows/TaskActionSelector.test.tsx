import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { WorkflowTask } from "@/types/workflow";
import TaskActionSelector from "./TaskActionSelector";

describe("finite target controls", () => {
  it("accepts exactly 256 literal targets and prevents adding a 257th", () => {
    const onChange = vi.fn();
    render(
      <TaskActionSelector
        task={{
          action: "{{ parameters.target }}",
          allowed_action_refs: Array.from(
            { length: 256 },
            (_, index) => `example.action${index}`,
          ),
        }}
        path="/tasks/0"
        availableActions={[]}
        onChange={onChange}
      />,
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Add action target" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Remove action target 256" }),
    );
    expect(onChange).toHaveBeenCalledWith({
      allowed_action_refs: Array.from(
        { length: 255 },
        (_, index) => `example.action${index}`,
      ),
    });
  });

  it("allows clearing an optional literal ceiling but never silently clears it when the selector changes", () => {
    function Editor() {
      const [task, setTask] = useState<
        Pick<WorkflowTask, "action" | "allowed_action_refs">
      >({ action: "example.one", allowed_action_refs: ["example.one"] });
      return (
        <TaskActionSelector
          task={task}
          path="/tasks/0"
          availableActions={[]}
          onChange={(updates) =>
            setTask((previous) => ({ ...previous, ...updates }))
          }
        />
      );
    }
    render(<Editor />);
    fireEvent.change(screen.getByLabelText("Action selector"), {
      target: { value: "example.other" },
    });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "/tasks/0/action: literal action must belong",
    );
    expect(screen.getByLabelText("Allowed action target 1")).toHaveValue(
      "example.one",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Remove optional target ceiling" }),
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Action selector")).toHaveValue(
      "example.other",
    );
  });
});
