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
