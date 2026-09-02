import { describe, expect, it, vi, beforeEach } from "vitest";
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { CacheNamespaceFreshness, OwnerType } from "@/api";
import {
  cacheKeys,
  useCacheEntryScan,
  useCacheNamespaces,
  usePromoteCacheGeneration,
} from "@/hooks/useCaches";

const listNamespaces = vi.fn();
const scanEntries = vi.fn();
const promoteGeneration = vi.fn();

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return {
    ...actual,
    CachesService: {
      listNamespaces: (...args: unknown[]) => listNamespaces(...args),
      scanEntries: (...args: unknown[]) => scanEntries(...args),
      promoteGeneration: (...args: unknown[]) => promoteGeneration(...args),
    },
  };
});

function createQueryHarness() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    queryClient,
    wrapper: function Wrapper({ children }: { children: ReactNode }) {
      return (
        <QueryClientProvider client={queryClient}>
          {children}
        </QueryClientProvider>
      );
    },
  };
}

function createWrapper() {
  return createQueryHarness().wrapper;
}

const owner = { ownerType: OwnerType.PACK, ownerRef: "salesforce" };
const ownerScope = { kind: "owner", owner } as const;

beforeEach(() => {
  listNamespaces.mockReset();
  scanEntries.mockReset();
  promoteGeneration.mockReset();
});

describe("cacheKeys", () => {
  it("threads namespace through every derived key", () => {
    const namespaceKey = cacheKeys.namespace(owner, "users");
    expect(namespaceKey).toEqual([
      "caches",
      "namespace",
      OwnerType.PACK,
      "salesforce",
      "users",
    ]);
    expect(cacheKeys.generations(owner, "users")).toEqual([
      ...namespaceKey,
      "generations",
    ]);
    expect(cacheKeys.generation(owner, "users", 42)).toEqual([
      ...namespaceKey,
      "generations",
      42,
    ]);
  });

  it("includes namespace, generation, cursor, and page shape in the entry-scan key", () => {
    const key = cacheKeys.entryScan(owner, "users", {
      generationId: 12345,
      cursor: "cursor-abc",
      limit: 50,
    });
    expect(key).toEqual([
      "caches",
      "namespace",
      OwnerType.PACK,
      "salesforce",
      "users",
      "entries",
      "scan",
      "generation",
      12345,
      "cursor-abc",
      50,
      false,
    ]);
  });

  it("produces a distinct key for a different generation, cursor, page size, or freshness shape", () => {
    const base = cacheKeys.entryScan(owner, "users", {
      generationId: 1,
      cursor: "a",
      limit: 50,
    });
    const differentGeneration = cacheKeys.entryScan(owner, "users", {
      generationId: 2,
      cursor: "a",
      limit: 50,
    });
    const differentCursor = cacheKeys.entryScan(owner, "users", {
      generationId: 1,
      cursor: "b",
      limit: 50,
    });
    const differentPageShape = cacheKeys.entryScan(owner, "users", {
      generationId: 1,
      cursor: "a",
      limit: 100,
    });
    const differentFreshnessShape = cacheKeys.entryScan(owner, "users", {
      generationId: 1,
      cursor: "a",
      limit: 50,
      requireFresh: true,
    });

    expect(differentGeneration).not.toEqual(base);
    expect(differentCursor).not.toEqual(base);
    expect(differentPageShape).not.toEqual(base);
    expect(differentFreshnessShape).not.toEqual(base);
  });

  it("uses a distinct current-generation branch and normalizes optional page shape", () => {
    const key = cacheKeys.entryScan(owner, "users", {});
    expect(key).toEqual([
      ...cacheKeys.currentEntryScans(owner, "users"),
      null,
      null,
      false,
    ]);
  });
});

describe("useCacheNamespaces", () => {
  it("lists every accessible owner when the browse scope is all", async () => {
    listNamespaces.mockResolvedValue({
      data: { namespaces: [], next_cursor: null },
    });

    const { result } = renderHook(() => useCacheNamespaces({ kind: "all" }), {
      wrapper: createWrapper(),
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(listNamespaces).toHaveBeenCalledWith({});
  });

  it("forwards a concrete owner to CachesService.listNamespaces", async () => {
    listNamespaces.mockResolvedValue({
      data: { namespaces: [], next_cursor: null },
    });

    const { result } = renderHook(() => useCacheNamespaces(ownerScope), {
      wrapper: createWrapper(),
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(listNamespaces).toHaveBeenCalledWith({
      ownerType: owner.ownerType,
      ownerRef: owner.ownerRef,
    });
  });

  it("includes server-side filters and cursor shape in the request and query key", async () => {
    listNamespaces.mockResolvedValue({
      data: { namespaces: [], next_cursor: "next-page" },
    });
    const shape = {
      namespace: "sales",
      freshness: CacheNamespaceFreshness.STALE,
      limit: 50,
      cursor: "cursor-1",
    };
    const { result } = renderHook(() => useCacheNamespaces(ownerScope, shape), {
      wrapper: createWrapper(),
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(listNamespaces).toHaveBeenCalledWith({
      ownerType: owner.ownerType,
      ownerRef: owner.ownerRef,
      ...shape,
    });
    expect(cacheKeys.list(ownerScope, shape)).toEqual([
      "caches",
      "list",
      "owner",
      OwnerType.PACK,
      "salesforce",
      "sales",
      CacheNamespaceFreshness.STALE,
      50,
      "cursor-1",
    ]);
  });

  it("does not fetch while a component owner selection is incomplete", () => {
    const { result } = renderHook(
      () => useCacheNamespaces({ kind: "incomplete" }),
      {
        wrapper: createWrapper(),
      },
    );

    expect(result.current.fetchStatus).toBe("idle");
    expect(listNamespaces).not.toHaveBeenCalled();
  });

  it("uses distinct query keys for all, scoped, and incomplete browsing", () => {
    expect(cacheKeys.list({ kind: "all" })).not.toEqual(
      cacheKeys.list(ownerScope),
    );
    expect(cacheKeys.list({ kind: "incomplete" })).not.toEqual(
      cacheKeys.list({ kind: "all" }),
    );
  });
});

describe("useCacheEntryScan", () => {
  it("does not fetch until explicitly enabled (deliberate access)", () => {
    const { result } = renderHook(
      () =>
        useCacheEntryScan(owner, "users", {
          limit: 50,
          enabled: false,
        }),
      { wrapper: createWrapper() },
    );

    expect(result.current.fetchStatus).toBe("idle");
    expect(scanEntries).not.toHaveBeenCalled();
  });

  it("fetches once enabled, pinning the returned generation", async () => {
    scanEntries.mockResolvedValue({
      data: {
        generation_id: 777,
        stale: false,
        items: [],
        next_cursor: null,
        cursor_expires_at: null,
        record_count: null,
      },
    });

    const { result } = renderHook(
      () =>
        useCacheEntryScan(owner, "users", {
          limit: 50,
          enabled: true,
        }),
      { wrapper: createWrapper() },
    );

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(scanEntries).toHaveBeenCalledWith(
      expect.objectContaining({
        ownerType: owner.ownerType,
        ownerRef: owner.ownerRef,
        namespace: "users",
        limit: 50,
      }),
    );
    expect(result.current.data?.data.generation_id).toBe(777);
  });
});

describe("usePromoteCacheGeneration", () => {
  it("resets current-generation scans without invalidating pinned traversal pages", async () => {
    promoteGeneration.mockResolvedValue({ data: { generation_id: 2 } });
    scanEntries.mockResolvedValue({
      data: {
        generation_id: 2,
        stale: false,
        items: [],
        next_cursor: null,
        cursor_expires_at: null,
        record_count: 0,
      },
    });
    const { queryClient, wrapper } = createQueryHarness();
    const currentKey = cacheKeys.entryScan(owner, "users", { limit: 50 });
    const pinnedKey = cacheKeys.entryScan(owner, "users", {
      generationId: 1,
      cursor: "next",
      limit: 50,
    });
    queryClient.setQueryData(currentKey, { data: { generation_id: 1 } });
    queryClient.setQueryData(pinnedKey, { data: { generation_id: 1 } });

    const { result } = renderHook(
      () => usePromoteCacheGeneration(owner, "users"),
      { wrapper },
    );

    result.current.mutate({
      generationId: 2,
      data: { expected_active_generation_id: 1 },
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(queryClient.getQueryState(currentKey)?.data).toBeUndefined();
    expect(queryClient.getQueryState(pinnedKey)?.isInvalidated).toBe(false);

    const currentScan = renderHook(
      () =>
        useCacheEntryScan(owner, "users", {
          limit: 50,
          enabled: true,
        }),
      { wrapper },
    );

    await waitFor(() =>
      expect(currentScan.result.current.data?.data.generation_id).toBe(2),
    );
    expect(scanEntries).toHaveBeenCalledTimes(1);
  });
});
