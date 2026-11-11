import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import WorkflowInputsPanel from "./WorkflowInputsPanel";
import type { ParamDefinition } from "@/types/workflow";

vi.mock("@/components/common/SchemaTypePicker", () => ({
  default: () => null,
}));

function panel(override?: Record<string, ParamDefinition>) {
  const output =
    override ??
    ({
      account: {
        type: "object",
        properties: {
          token: { type: "string", secret: true },
          name: { type: "string" },
        },
        required: ["token"],
      },
    } as unknown as Record<string, ParamDefinition>);
  const onOutputChange = vi.fn();
  const onOutputMapChange = vi.fn();
  render(
    <WorkflowInputsPanel
      label="Example"
      version="1.0.0"
      description=""
      tags={[]}
      referenceVisibility="public"
      referenceAllowedPackRefs={[]}
      cancellationPolicy="allow_finish"
      parameters={{}}
      output={output}
      outputMap={{ account: "{{ task.fetch.result.data.account }}" }}
      outputPolicy={{}}
      onOutputPolicyChange={vi.fn()}
      onLabelChange={vi.fn()}
      onVersionChange={vi.fn()}
      onDescriptionChange={vi.fn()}
      onTagsChange={vi.fn()}
      onReferenceVisibilityChange={vi.fn()}
      onReferenceAllowedPackRefsChange={vi.fn()}
      onCancellationPolicyChange={vi.fn()}
      onParametersChange={vi.fn()}
      onOutputChange={onOutputChange}
      onOutputMapChange={onOutputMapChange}
    />,
  );
  return { output, onOutputChange, onOutputMapChange };
}

describe("workflow output schema editing", () => {
  it("preserves nested secret annotations and child requiredness when editing an expression", () => {
    const { output, onOutputChange, onOutputMapChange } = panel();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.change(
      screen.getByPlaceholderText("{{ task.fetch.result.data.headline }}"),
      {
        target: { value: "{{ task.fetch.result.data.updated_account }}" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onOutputChange).toHaveBeenCalledWith(output);
    expect(onOutputMapChange).toHaveBeenCalledWith({
      account: "{{ task.fetch.result.data.updated_account }}",
    });
  });

  it("supports nested schema edits without dropping output expressions", () => {
    const { onOutputChange, onOutputMapChange } = panel();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(
      screen.getByRole("button", {
        name: "Edit nested output schema and secret fields",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Raw JSON" }));
    const edited = {
      account: {
        type: "object",
        properties: {
          token: { type: "string", secret: true },
          name: { type: "string" },
          password: { type: "string", secret: true },
        },
      },
    };
    fireEvent.change(
      screen.getByPlaceholderText(
        '{"account": {"type": "object", "properties": {"token": {"type": "string", "secret": true}}}}',
      ),
      {
        target: { value: JSON.stringify(edited) },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onOutputChange).toHaveBeenCalledWith(edited);
    expect(onOutputMapChange).toHaveBeenCalledWith({
      account: "{{ task.fetch.result.data.account }}",
    });
  });
  it.each([
    {
      type: ["object", "null"],
      properties: { token: { type: "string", secret: true } },
    },
    {
      anyOf: [
        {
          type: "object",
          properties: { token: { type: "string", secret: true } },
        },
        { type: "null" },
      ],
    },
  ])(
    "preserves compound type declarations during expression edits",
    (account) => {
      const output = { account } as unknown as Record<string, ParamDefinition>;
      const { onOutputChange } = panel(output);
      fireEvent.click(screen.getByRole("button", { name: "Edit" }));
      fireEvent.change(
        screen.getByPlaceholderText("{{ task.fetch.result.data.headline }}"),
        {
          target: { value: "{{ task.fetch.result.data.updated_account }}" },
        },
      );
      fireEvent.click(screen.getByRole("button", { name: "Apply" }));
      expect(onOutputChange).toHaveBeenCalledWith(output);
    },
  );
});
