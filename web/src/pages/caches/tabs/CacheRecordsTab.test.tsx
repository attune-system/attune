import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { OwnerType } from "@/api/cache";
import { ApiError } from "@/api/core/ApiError";
import CacheRecordsTab from "@/pages/caches/tabs/CacheRecordsTab";

const scanEntries = vi.fn();

vi.mock("@/api/cache", async () => {
  const actual =
    await vi.importActual<typeof import("@/api/cache")>("@/api/cache");
  return {
    ...actual,
    CachesService: {
      scanEntries: (...args: unknown[]) => scanEntries(...args),
    },
  };
});

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  };
}

function scanPage(generationId: number, nextCursor: string | null = null) {
  return {
    data: {
      generation_id: generationId,
      items: [],
      next_cursor: nextCursor,
      cursor_expires_at: null,
      record_count: 0,
      stale: false,
    },
  };
}

function snapshotExpiredError() {
  return new ApiError(
    { method: "GET", url: "/cache" },
    {
      url: "/cache",
      ok: false,
      status: 409,
      statusText: "Conflict",
      body: { code: "snapshot_expired" },
    },
    "snapshot expired",
  );
}

beforeEach(() => {
  scanEntries.mockReset();
});

describe("CacheRecordsTab", () => {
  it("refetches current generation after an expired pinned traversal", async () => {
    const user = userEvent.setup();
    scanEntries
      .mockResolvedValueOnce(scanPage(101, "cursor-101"))
      .mockRejectedValueOnce(snapshotExpiredError())
      .mockResolvedValueOnce(scanPage(202));

    render(
      <CacheRecordsTab
        owner={{ ownerType: OwnerType.PACK, ownerRef: "salesforce" }}
        namespaceName="users"
      />,
      { wrapper: createWrapper() },
    );

    await user.click(screen.getByRole("button", { name: "Start browsing" }));
    expect(
      await screen.findByText("Pinned generation #101"),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(
      await screen.findByText("This browsing snapshot has expired."),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", {
        name: "Restart on current generation",
      }),
    );

    expect(
      await screen.findByText("Pinned generation #202"),
    ).toBeInTheDocument();
    await waitFor(() => expect(scanEntries).toHaveBeenCalledTimes(3));
    expect(scanEntries).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({
        generationId: 101,
        cursor: "cursor-101",
      }),
    );
    expect(scanEntries).toHaveBeenNthCalledWith(
      3,
      expect.objectContaining({
        generationId: undefined,
        cursor: undefined,
      }),
    );
  });
});
