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
