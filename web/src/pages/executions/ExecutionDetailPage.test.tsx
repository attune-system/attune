import { act, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ExecutionStatus,
  LogAvailabilityState,
  ExecutionsService,
  type ExecutionResponse,
} from "@/api";
import ExecutionDetailPage from "./ExecutionDetailPage";

const state = vi.hoisted(() => ({
  signedOut: false,
  user: {
    id: 1,
    effective_permissions: [{ resource: "executions", actions: ["decrypt"] }],
  },
  execution: undefined as ExecutionResponse | undefined,
}));
vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ user: state.signedOut ? null : state.user }),
}));
vi.mock("@/hooks/useExecutions", () => ({
  useExecution: () => ({ data: { data: state.execution } }),
  useCancelExecution: () => ({}),
  useRescheduleExecution: () => ({}),
}));
vi.mock("@/hooks/useActions", () => ({ useAction: () => ({}) }));
vi.mock("@/hooks/usePermissions", () => ({
  useIdentity: () => ({}),
  usePermissionSets: () => ({}),
}));
vi.mock("@/hooks/useWorkers", () => ({ useWorker: () => ({}) }));
vi.mock("@/hooks/useEvents", () => ({
  useEnforcement: () => ({}),
  useEvent: () => ({}),
}));
vi.mock("@/hooks/useHistory", () => ({ useExecutionHistory: () => ({}) }));
vi.mock("@/hooks/useExecutionStream", () => ({
  useExecutionStream: () => ({}),
}));
vi.mock("@/components/common/EntityHistoryPanel", () => ({
  default: () => null,
}));
vi.mock("@/components/executions/ExecutionArtifactsPanel", () => ({
  default: () => null,
}));

const marker = { $attune_secret: true, redacted: true };
const execution: ExecutionResponse = {
  id: 42,
  action_ref: "example.credentials",
  created: "2026-11-09T00:00:00Z",
  updated: "2026-11-09T00:00:01Z",
  status: ExecutionStatus.COMPLETED,
  config: { password: marker },
  result: {
    data: { token: marker, restricted: marker },
    stdout: "public-sanitized-stdout",
    output_channels: {
      stdout: {
        policy: "sanitized",
        original: { execution_id: 42, channel: "stdout" },
      },
    },
  },
  schema_binding: {
    input: {
      annotated_schema: {
        type: "object",
        properties: { password: { type: "string", secret: true } },
      },
    },
    output: {
      annotated_schema: {
        type: "object",
        properties: { token: { type: "string", secret: true } },
      },
    },
  },
};
const revealed: ExecutionResponse = {
  ...execution,
  config: { password: "private-password" },
  result: {
    data: { token: "private-token", restricted: marker },
    stdout: "public-sanitized-stdout",
  },
  revealed_output_channels: { stdout: "private-stdout" },
};

function Page() {
  return (
    <MemoryRouter initialEntries={["/executions/42"]}>
      <Routes>
        <Route path="/executions/:id" element={<ExecutionDetailPage />} />
      </Routes>
    </MemoryRouter>
  );
}

function show() {
  fireEvent.click(screen.getByRole("button", { name: "Show secret values" }));
}

beforeEach(() => {
  vi.restoreAllMocks();
  state.signedOut = false;
  state.user = {
    id: 1,
    effective_permissions: [{ resource: "executions", actions: ["decrypt"] }],
  };
  state.execution = execution;
});

describe("execution secret disclosure", () => {
  it("only fetches plaintext on request, renders bound secret fields, and hides it again", async () => {
    const get = vi
      .spyOn(ExecutionsService, "getExecution")
      .mockResolvedValue({ data: revealed });
    render(<Page />);
    expect(get).not.toHaveBeenCalled();
    expect(screen.getByText("public-sanitized-stdout")).toBeInTheDocument();
    expect(screen.queryByText(/private-token/)).not.toBeInTheDocument();
    show();
    expect(await screen.findByText(/private-token/)).toBeInTheDocument();
    expect(screen.getByText(/private-password/)).toBeInTheDocument();
    expect(screen.getByText(/private-stdout/)).toBeInTheDocument();
    expect(screen.getAllByText("[REDACTED]").length).toBeGreaterThan(0);
    expect(get).toHaveBeenCalledExactlyOnceWith({
      id: 42,
      includeSecretValues: true,
    });
    fireEvent.click(screen.getByRole("button", { name: "Hide secret values" }));
    expect(screen.queryByText(/private-token/)).not.toBeInTheDocument();
    expect(screen.queryByText(/private-password/)).not.toBeInTheDocument();
    expect(screen.queryByText(/private-stdout/)).not.toBeInTheDocument();
    expect(execution.config.password).toEqual(marker);
    expect(execution.result.data.token).toEqual(marker);
    show();
    await screen.findByText(/private-token/);
    expect(get).toHaveBeenCalledTimes(2);
  });

  it("does not offer disclosure without execution decrypt permission", () => {
    state.user.effective_permissions = [
      { resource: "executions", actions: ["read"] },
    ];
    const get = vi.spyOn(ExecutionsService, "getExecution");
    render(<Page />);
    expect(
      screen.queryByRole("button", { name: "Show secret values" }),
    ).not.toBeInTheDocument();
    expect(get).not.toHaveBeenCalled();
  });

  it.each([403, 500])(
    "keeps values redacted when the server rejects disclosure (%s)",
    async (status) => {
      vi.spyOn(ExecutionsService, "getExecution").mockRejectedValue({
        status,
        body: "private-error-content",
      });
      render(<Page />);
      show();
      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toContain(
        status === 403 ? "do not have permission" : "could not be loaded",
      );
      expect(
        screen.queryByText("private-error-content"),
      ).not.toBeInTheDocument();
      expect(screen.queryByText(/private-token/)).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Show secret values" }),
      ).toBeInTheDocument();
    },
  );

  it("discards a pending response after cancelling", async () => {
    let complete!: (value: { data: ExecutionResponse }) => void;
    vi.spyOn(ExecutionsService, "getExecution").mockImplementation(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }) as ReturnType<typeof ExecutionsService.getExecution>,
    );
    render(<Page />);
    show();
    expect(screen.getByRole("status")).toHaveTextContent(
      "Loading secret values",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Cancel revealing secret values" }),
    );
    await act(async () => complete({ data: revealed }));
    expect(screen.queryByText(/private-token/)).not.toBeInTheDocument();
  });

  it.each(["navigation", "sign-out", "unmount"])(
    "discards a pending response after %s",
    async (change) => {
      let complete!: (value: { data: ExecutionResponse }) => void;
      vi.spyOn(ExecutionsService, "getExecution").mockImplementation(
        () =>
          new Promise((resolve) => {
            complete = resolve;
          }) as ReturnType<typeof ExecutionsService.getExecution>,
      );
      const view = render(<Page />);
      show();
      if (change === "navigation") state.execution = { ...execution, id: 43 };
      if (change === "sign-out") state.signedOut = true;
      if (change === "unmount") view.unmount();
      else view.rerender(<Page />);
      await act(async () => complete({ data: revealed }));
      if (change === "unmount") render(<Page />);
      expect(screen.queryByText(/private-token/)).not.toBeInTheDocument();
      expect(screen.queryByText(/private-password/)).not.toBeInTheDocument();
    },
  );

  it.each(["execution", "identity", "permissions", "refresh"])(
    "clears disclosure when %s changes",
    async (change) => {
      vi.spyOn(ExecutionsService, "getExecution").mockResolvedValue({
        data: revealed,
      });
      const view = render(<Page />);
      show();
      await screen.findByText(/private-token/);
      if (change === "execution") state.execution = { ...execution, id: 43 };
      if (change === "identity") state.user = { ...state.user, id: 2 };
      if (change === "permissions")
        state.user = { ...state.user, effective_permissions: [] };
      if (change === "refresh")
        state.execution = { ...execution, updated: "2026-11-09T00:00:02Z" };
      view.rerender(<Page />);
      expect(screen.queryByText(/private-token/)).not.toBeInTheDocument();
      expect(screen.queryByText(/private-password/)).not.toBeInTheDocument();
    },
  );

  it("does not carry disclosure to a remounted execution view", async () => {
    vi.spyOn(ExecutionsService, "getExecution").mockResolvedValue({
      data: revealed,
    });
    const view = render(<Page />);
    show();
    await screen.findByText(/private-token/);
    view.unmount();
    render(<Page />);
    expect(screen.queryByText(/private-token/)).not.toBeInTheDocument();
  });
});

describe("completed executions with delayed logs", () => {
  it("keeps the action result visible while log availability settles", () => {
    const delivery = {
      state: LogAvailabilityState.FINALIZING,
      artifact_version: 71,
      stream: 72,
      revision: 2,
      finalize_by: "2026-11-12T10:00:00Z",
      retry_after_ms: 500,
    };
    state.execution = { ...execution, log_delivery: { stdout: delivery } };
    const view = render(<Page />);
    expect(
      screen.getByText("Execution finished. Logs are still finalizing."),
    ).toBeInTheDocument();
    expect(screen.getByText("public-sanitized-stdout")).toBeInTheDocument();
    state.execution = {
      ...state.execution,
      log_delivery: {
        stdout: { ...delivery, state: LogAvailabilityState.READY },
      },
    };
    view.rerender(<Page />);
    expect(
      screen.queryByText("Execution finished. Logs are still finalizing."),
    ).not.toBeInTheDocument();
    expect(screen.getByText("public-sanitized-stdout")).toBeInTheDocument();
  });
});
