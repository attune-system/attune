import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import RunWorkflowModal from "./RunWorkflowModal";

const { execute } = vi.hoisted(() => ({ execute: vi.fn() }));
vi.mock("@/hooks/useExecutions", () => ({
  useRequestExecution: () => ({ mutateAsync: execute }),
}));

describe("workflow manual admission", () => {
  beforeEach(() => {
    execute.mockReset();
    vi.spyOn(window, "open").mockReturnValue(null);
  });
  afterEach(() => vi.restoreAllMocks());

  it("preserves explicit empty strings, null, false, and zero", async () => {
    execute.mockResolvedValue({ data: { id: 42 } });
    const save = vi.fn().mockResolvedValue(true);
    const close = vi.fn();
    render(
      <RunWorkflowModal
        actionRef="fixture.workflow"
        paramSchema={{
          text: { type: "string", default: "" },
          nullable: { default: null },
          enabled: { type: "boolean", default: false },
          count: { type: "integer", default: 0 },
        }}
        onSave={save}
        onClose={close}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(execute).toHaveBeenCalledWith({
      actionRef: "fixture.workflow",
      parameters: { text: "", nullable: null, enabled: false, count: 0 },
    });
    expect(save).toHaveBeenCalledOnce();
  });

  it("warns after a changed preview without selectors or a duplicate execution", async () => {
    vi.spyOn(SchemaTypesService, "previewSchema").mockReturnValue(
      new CancelablePromise<
        Awaited<ReturnType<typeof SchemaTypesService.previewSchema>>
      >((resolve) =>
        resolve({
          data: {
            semantic_digest: "preview",
            contract: {
              annotated_schema: {
                type: "object",
                properties: { value: { type: "string" } },
              },
            },
          },
        }),
      ),
    );
    execute.mockResolvedValue({
      data: { id: 43, schema_binding: { input_digest: "accepted" } },
    });
    const close = vi.fn();
    render(
      <RunWorkflowModal
        actionRef="fixture.workflow"
        paramSchema={{ value: { type: "fixture.value" } }}
        onSave={vi.fn().mockResolvedValue(true)}
        onClose={close}
      />,
    );
    await waitFor(() =>
      expect(screen.queryByText("Resolving schema...")).not.toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    await screen.findByText(/accepted input contract differs/);
    expect(execute).toHaveBeenCalledExactlyOnceWith({
      actionRef: "fixture.workflow",
      parameters: {},
    });
    expect(screen.getByRole("button", { name: "Run" })).toBeDisabled();
    expect(close).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(close).toHaveBeenCalledOnce();
  });

  it("does not display secret-bearing exception messages", async () => {
    execute.mockRejectedValue(new Error("secret-error-fixture"));
    render(
      <RunWorkflowModal
        actionRef="fixture.workflow"
        paramSchema={{}}
        onSave={vi.fn().mockResolvedValue(true)}
        onClose={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    await screen.findByText("Failed to start execution");
    expect(screen.queryByText(/secret-error-fixture/)).not.toBeInTheDocument();
  });
});
