import { useState, type ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useArtifactLivePage } from "./useArtifactLivePage";

describe("live artifact offset pagination", () => {
  const disposables: (() => void)[] = [];
  afterEach(() => disposables.splice(0).forEach((dispose) => dispose()));

  function browse(paused = false, initialPage = 2) {
    const client = new QueryClient();
    const first = ["artifacts", "list", { page: 1 }];
    const second = ["artifacts", "list", { page: 2 }];
    client.setQueryData(first, { items: ["old first-page rows"] });
    client.setQueryData(second, { items: ["old second-page rows"] });
    client.setQueryData(["artifacts", 42], { name: "unrelated detail" });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const hook = renderHook(
      ({ paused }) => {
        const [page, setPage] = useState(initialPage);
        useArtifactLivePage({ scope: "list", page, setPage, paused });
        return { page, setPage };
      },
      { wrapper, initialProps: { paused } },
    );
    disposables.push(() => {
      hook.unmount();
      client.clear();
    });
    return { client, first, second, ...hook };
  }

  it.each(["insertion", "retention deletion", "reconciliation", "resume"])(
    "resets on an affected %s refresh without retaining old page results",
    async () => {
      const { client, first, second, result } = browse();
      await act(() =>
        client.invalidateQueries({
          queryKey: ["artifacts", "list"],
          refetchType: "none",
        }),
      );
      expect(result.current.page).toBe(1);
      expect(client.getQueryData(first)).toBeUndefined();
      expect(client.getQueryData(second)).toBeUndefined();
      expect(client.getQueryData(["artifacts", 42])).toEqual({
        name: "unrelated detail",
      });
    },
  );

  it("preserves paused browsing, then resets when resumed refresh arrives", async () => {
    const { client, second, result, rerender } = browse(true);
    await act(() =>
      client.invalidateQueries({ queryKey: second, refetchType: "none" }),
    );
    expect(result.current.page).toBe(2);
    expect(client.getQueryData(second)).toBeDefined();
    // Reset on resume even if a previous invalidation is still pending and
    // repeating query.invalidate() would emit no new event.
    rerender({ paused: false });
    expect(result.current.page).toBe(1);
  });

  it("discards inactive older pages while staying on page one", async () => {
    const { client, first, second, result } = browse(false, 1);
    await act(() =>
      client.invalidateQueries({ queryKey: first, refetchType: "none" }),
    );
    expect(result.current.page).toBe(1);
    expect(client.getQueryData(second)).toBeUndefined();
    expect(client.getQueryData(first)).toBeDefined();
  });

  it("does not reset on an unrelated query or ordinary page navigation", async () => {
    const { client, result } = browse();
    await act(() =>
      client.invalidateQueries({
        queryKey: ["artifacts", 42],
        refetchType: "none",
      }),
    );
    expect(result.current.page).toBe(2);
    act(() => result.current.setPage(3));
    expect(result.current.page).toBe(3);
  });

  it("unsubscribes on unmount", async () => {
    const { client, second, unmount } = browse();
    unmount();
    await client.invalidateQueries({ queryKey: second, refetchType: "none" });
    expect(client.getQueryData(second)).toBeDefined();
  });

  it("keeps progress-discovery queries when resetting execution browse pages", async () => {
    const client = new QueryClient();
    const key = ["artifacts", "execution", 42];
    const progress = [...key, { type: "progress", perPage: 1 }];
    const oldPage = [...key, { page: 2, perPage: 20 }];
    client.setQueryData(progress, { items: ["progress summary"] });
    client.setQueryData(oldPage, { items: ["older browse"] });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const hook = renderHook(
      () => {
        const [page, setPage] = useState(2);
        useArtifactLivePage({
          scope: "execution",
          id: 42,
          page,
          setPage,
          paused: false,
        });
        return page;
      },
      { wrapper },
    );
    disposables.push(() => {
      hook.unmount();
      client.clear();
    });
    await act(() =>
      client.invalidateQueries({ queryKey: key, refetchType: "none" }),
    );
    expect(hook.result.current).toBe(1);
    expect(client.getQueryData(oldPage)).toBeUndefined();
    expect(client.getQueryData(progress)).toEqual({
      items: ["progress summary"],
    });
  });

  it.each([
    { scope: "execution", key: ["artifacts", "execution", 42] },
    { scope: "versions", key: ["artifacts", 42, "versions"] },
  ] satisfies Array<{
    scope: "execution" | "versions";
    key: Array<string | number>;
  }>)("resets $scope pages on affected refreshes", async ({ scope, key }) => {
    const client = new QueryClient();
    const old = [...key, { page: 2 }];
    client.setQueryData(old, { items: ["old rows"] });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const hook = renderHook(
      () => {
        const [page, setPage] = useState(2);
        useArtifactLivePage({ scope, id: 42, page, setPage, paused: false });
        return page;
      },
      { wrapper },
    );
    disposables.push(() => {
      hook.unmount();
      client.clear();
    });
    await act(() =>
      client.invalidateQueries({ queryKey: key, refetchType: "none" }),
    );
    expect(hook.result.current).toBe(1);
    expect(client.getQueryData(old)).toBeUndefined();
  });

  it("mounts a fresh first-page query rather than refetching the old active page", async () => {
    const client = new QueryClient();
    for (const page of [1, 2])
      client.setQueryData(["artifacts", "list", { page }], {
        page,
        fresh: false,
      });
    const fetch = vi.fn(async (page: number) => ({ page, fresh: true }));
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const hook = renderHook(
      () => {
        const [page, setPage] = useState(2);
        const query = useQuery({
          queryKey: ["artifacts", "list", { page }],
          queryFn: () => fetch(page),
          staleTime: Infinity,
        });
        useArtifactLivePage({ scope: "list", page, setPage, paused: false });
        return { page, data: query.data };
      },
      { wrapper },
    );
    disposables.push(() => {
      hook.unmount();
      client.clear();
    });
    expect(hook.result.current.data).toEqual({ page: 2, fresh: false });
    await act(() =>
      client.invalidateQueries({ queryKey: ["artifacts", "list"] }),
    );
    await waitFor(() =>
      expect(hook.result.current.data).toEqual({ page: 1, fresh: true }),
    );
    expect(fetch).toHaveBeenCalledExactlyOnceWith(1);
    expect(
      client.getQueryData(["artifacts", "list", { page: 2 }]),
    ).toBeUndefined();
  });
});
