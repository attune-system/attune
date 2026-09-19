import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  usePackReleases,
  usePlatformCatalog,
  useRetiredPackComponents,
} from "@/hooks/usePacks";

const getPackReleases = vi.fn();
const getRetiredPackComponents = vi.fn();
const getPlatformCatalog = vi.fn();

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return {
    ...actual,
    PacksService: {
      ...actual.PacksService,
      getPackReleases: (...args: unknown[]) => getPackReleases(...args),
      getRetiredPackComponents: (...args: unknown[]) =>
        getRetiredPackComponents(...args),
      getPlatformCatalog: (...args: unknown[]) => getPlatformCatalog(...args),
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

beforeEach(() => {
  getPackReleases.mockReset();
  getRetiredPackComponents.mockReset();
  getPlatformCatalog.mockReset();
});

describe("pack lifecycle queries", () => {
  it("loads release history for the selected pack", async () => {
    getPackReleases.mockResolvedValue({ data: [] });
    const { result } = renderHook(() => usePackReleases("core"), {
      wrapper: createWrapper(),
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(getPackReleases).toHaveBeenCalledWith({ ref: "core" });
  });

  it("does not load retired components before the panel opens", async () => {
    getRetiredPackComponents.mockResolvedValue({ data: [] });
    const { result, rerender } = renderHook(
      ({ enabled }) => useRetiredPackComponents("core", enabled),
      { initialProps: { enabled: false }, wrapper: createWrapper() },
    );

    expect(result.current.fetchStatus).toBe("idle");
    expect(getRetiredPackComponents).not.toHaveBeenCalled();

    rerender({ enabled: true });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(getRetiredPackComponents).toHaveBeenCalledWith({ ref: "core" });
  });

  it("loads the global catalog state", async () => {
    getPlatformCatalog.mockResolvedValue({
      data: {
        compatibility_epoch: 1,
        revision: 1,
        expected_compatibility_epoch: 1,
        expected_revision: 1,
        status: "current",
      },
    });
    const { result } = renderHook(() => usePlatformCatalog(), {
      wrapper: createWrapper(),
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(getPlatformCatalog).toHaveBeenCalledOnce();
  });
});
