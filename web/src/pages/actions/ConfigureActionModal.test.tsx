import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ActionResponse } from "@/api";
import { ConfigureActionModal } from "./ActionsPage";

const api = vi.hoisted(() => ({ save: vi.fn() }));
vi.mock("@/hooks/useActions", () => ({
  useUpdateAction: () => ({ mutateAsync: api.save, isPending: false }),
}));
vi.mock("@/hooks/usePermissions", () => ({
  usePermissionSets: () => ({ data: [], isLoading: false }),
}));
vi.mock("@/components/common/SchemaTypePicker", () => ({
  default: () => null,
}));

const schema = {
  account: {
    type: "object",
    properties: { token: { type: "string", secret: true } },
  },
};
const action = {
  id: 42,
  ref: "example.run",
  label: "Run",
  entrypoint: "run.py",
  param_schema: { password: { type: "string", secret: true } },
  out_schema: schema,
  output_format: "json",
  output_policy: { stdout: "sanitized", stderr: "secret", error: "public" },
} as ActionResponse;

beforeEach(() => {
  api.save.mockReset().mockResolvedValue({});
});

describe("action output policy editing", () => {
  it("preserves nested output schema and policy through unrelated edits", async () => {
    render(<ConfigureActionModal action={action} onClose={() => {}} />);
    expect(screen.getByLabelText("Stdout")).toHaveValue("sanitized");
    fireEvent.click(screen.getByRole("button", { name: "Save Defaults" }));
    await waitFor(() => expect(api.save).toHaveBeenCalled());
    expect(api.save).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          out_schema: schema,
          output_format: "json",
          output_policy: action.output_policy,
          param_schema: action.param_schema,
        }),
      }),
    );
  });

  it("sends explicit channel edits and supports returning to schema defaults", async () => {
    render(<ConfigureActionModal action={action} onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText("Stdout"), {
      target: { value: "secret" },
    });
    fireEvent.change(screen.getByLabelText("Error text"), {
      target: { value: "" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save Defaults" }));
    await waitFor(() => expect(api.save).toHaveBeenCalled());
    expect(api.save).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          output_policy: { stdout: "secret", stderr: "secret" },
        }),
      }),
    );
  });
});

describe("binary launch editing", () => {
  const parameters = {
    output: {
      type: "string",
      default: "out file",
      cli: { kind: "positional", order: 20 },
    },
  };
  const launch = {
    type: "command",
    program: "tool",
    arguments: [{ literal: "generate", order: 0 }],
  };
  const binary = {
    ...action,
    entrypoint: null,
    launch,
    parameter_delivery: "none",
    parameter_format: "json",
    param_schema: parameters,
  } as ActionResponse;

  it("preserves launch, static tokens and CLI bindings through unrelated edits", async () => {
    render(<ConfigureActionModal action={binary} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Save Defaults" }));
    await waitFor(() => expect(api.save).toHaveBeenCalled());
    expect(api.save).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          entrypoint: null,
          launch,
          param_schema: parameters,
          parameter_delivery: "none",
          parameter_format: "json",
        }),
      }),
    );
  });

  it("clears the binary source explicitly when selecting an entry point", async () => {
    render(<ConfigureActionModal action={binary} onClose={() => {}} />);
    fireEvent.click(screen.getByLabelText("Use a binary launch"));
    fireEvent.change(screen.getByLabelText("Entry Point"), {
      target: { value: "run" },
    });
    fireEvent.change(screen.getByLabelText("Input parameter schema (JSON)"), {
      target: { value: "{}" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save Defaults" }));
    await waitFor(() => expect(api.save).toHaveBeenCalled());
    expect(api.save).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          entrypoint: "run",
          launch: null,
          param_schema: {},
          parameter_delivery: "stdin",
        }),
      }),
    );
  });

  it("preserves non-CLI native file delivery and format", async () => {
    const fileNative = {
      ...binary,
      launch: { type: "native", artifact: "tool", arguments: [] },
      param_schema: {},
      parameter_delivery: "file",
      parameter_format: "yaml",
    } as ActionResponse;
    render(<ConfigureActionModal action={fileNative} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Save Defaults" }));
    await waitFor(() => expect(api.save).toHaveBeenCalled());
    expect(api.save).toHaveBeenCalledWith(
      expect.objectContaining({
        data: expect.objectContaining({
          parameter_delivery: "file",
          parameter_format: "yaml",
        }),
      }),
    );
  });

  it("rejects unsupported source fields without saving a partial replacement", async () => {
    render(<ConfigureActionModal action={binary} onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText("Binary launch (JSON)"), {
      target: { value: JSON.stringify({ ...launch, extra_args: [] }) },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save Defaults" }));
    await screen.findByText("Launch contains unsupported fields");
    expect(api.save).not.toHaveBeenCalled();
  });
});
