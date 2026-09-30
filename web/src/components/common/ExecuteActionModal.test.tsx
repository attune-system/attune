import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ActionResponse } from "@/api";
import ExecuteActionModal from "./ExecuteActionModal";

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({
    user: { assigned_permission_set_refs: ["core.admin"] },
  }),
}));

vi.mock("@/hooks/usePermissions", () => ({
  usePermissionSets: () => ({ data: [] }),
}));

function CurrentPath() {
  return <div data-testid="current-path">{useLocation().pathname}</div>;
}

const action: ActionResponse = {
  accesses_mcp: false,
  created: "2026-09-16T00:00:00Z",
  enabled: true,
  entrypoint: "echo.sh",
  id: 15,
  is_adhoc: false,
  label: "Echo",
  out_schema: null,
  pack: 2,
  pack_ref: "core",
  param_schema: {},
  ref: "core.echo",
  reference_visibility: "public",
  updated: "2026-09-16T00:00:00Z",
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ExecuteActionModal", () => {
  it("shows a visible warning and blocks reserved environment names", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter>
          <ExecuteActionModal action={action} onClose={vi.fn()} />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    await userEvent.type(
      screen.getByPlaceholderText("Key"),
      "ATTUNE_API_TOKEN",
    );
    expect(screen.getByRole("alert")).toHaveTextContent("ATTUNE_API_TOKEN");
    expect(screen.getByRole("alert")).toHaveTextContent("internal use");
    expect(screen.getByRole("button", { name: "Execute" })).toBeDisabled();
    expect(fetchMock).not.toHaveBeenCalled();
    await userEvent.clear(screen.getByPlaceholderText("Key"));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Execute" })).toBeEnabled();
  });

  it("displays the API rejection and keeps the dialog open", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            error:
              "Environment variable 'ATTUNE_API_URL' uses the reserved ATTUNE_ prefix",
          }),
          { status: 400, headers: { "Content-Type": "application/json" } },
        ),
      ),
    );
    const onClose = vi.fn();
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter>
          <ExecuteActionModal action={action} onClose={onClose} />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Execute" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Execution was not created");
    expect(alert).toHaveTextContent("ATTUNE_API_URL");
    expect(onClose).not.toHaveBeenCalled();
  });

  it("navigates without reloading the application", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ data: { id: 42 } }), {
        status: 201,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={["/actions/core.echo"]}>
          <ExecuteActionModal action={action} onClose={vi.fn()} />
          <CurrentPath />
        </MemoryRouter>
      </QueryClientProvider>,
    );

    await userEvent.click(screen.getByRole("button", { name: "Execute" }));

    await waitFor(() => {
      expect(screen.getByTestId("current-path")).toHaveTextContent(
        "/executions/42",
      );
    });
  });
});
