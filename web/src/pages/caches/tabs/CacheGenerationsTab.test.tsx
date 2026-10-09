import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  CacheGenerationState,
  OwnerType,
  type CacheGenerationResponse,
} from "@/api";
import CacheGenerationsTab from "./CacheGenerationsTab";

const listGenerations = vi.hoisted(() => vi.fn());
const auth = vi.hoisted(() => ({ canReadExecutions: false }));
vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return { ...actual, CachesService: { listGenerations } };
});
vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ user: null }) }));
vi.mock("@/lib/permissions", () => ({
  hasPermission: (_user: unknown, resource: string, action: string) =>
    resource === "executions" && action === "read" && auth.canReadExecutions,
}));

const retiredGeneration: CacheGenerationResponse = {
  generation_id: 42,
  namespace_id: 1,
  status: CacheGenerationState.RETIRED,
  client_refresh_id: "original",
  created_by: null,
  created_by_execution: 9876543210,
  expected_active_generation_id: null,
  expected_chunk_count: 1,
  expected_record_count: 1,
  expected_size_bytes: null,
  source_revision: null,
  record_count: 1,
  size_bytes: 12,
  checksum: null,
  checksum_algorithm: null,
  created: "2026-10-07T00:00:00Z",
  sealed: null,
  activated: null,
  retired: "2026-10-07T01:00:00Z",
  readable_until: null,
  failed: null,
  failure_reason: null,
};

function createWrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <MemoryRouter>
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      </MemoryRouter>
    );
  };
}

beforeEach(() => {
  auth.canReadExecutions = false;
  listGenerations.mockReset();
  listGenerations.mockResolvedValue({
    data: { generations: [retiredGeneration], next_cursor: null },
  });
});

describe("CacheGenerationsTab", () => {
  it.each([false, true])(
    "shows historical attribution with execution read = %s",
    async (canReadExecutions) => {
      auth.canReadExecutions = canReadExecutions;
      const user = userEvent.setup();
      render(
        <CacheGenerationsTab
          owner={{ ownerType: OwnerType.SYSTEM }}
          namespaceName="users"
        />,
        { wrapper: createWrapper() },
      );
      await user.click(await screen.findByText("#42"));
      expect(screen.getByText("Execution #9876543210")).toBeInTheDocument();
      expect(
        screen.getByText(/Historical attribution, not live execution status/),
      ).toBeInTheDocument();
      if (canReadExecutions) {
        expect(
          screen.getByRole("link", { name: "Execution #9876543210" }),
        ).toHaveAttribute("href", "/executions/9876543210");
      } else {
        expect(
          screen.queryByRole("link", { name: "Execution #9876543210" }),
        ).not.toBeInTheDocument();
      }
    },
  );

  it("does not invent execution attribution for a manual producer", async () => {
    listGenerations.mockResolvedValue({
      data: {
        generations: [{ ...retiredGeneration, created_by_execution: null }],
        next_cursor: null,
      },
    });
    const user = userEvent.setup();
    render(
      <CacheGenerationsTab
        owner={{ ownerType: OwnerType.SYSTEM }}
        namespaceName="users"
      />,
      { wrapper: createWrapper() },
    );
    await user.click(await screen.findByText("#42"));
    expect(screen.getByText("None recorded")).toBeInTheDocument();
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });
});
