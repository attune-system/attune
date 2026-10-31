import { useState, type ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { request } from "@/api/core/request";
import { useArtifactVersions, useExecutionArtifacts } from "./useArtifacts";
import { queryClient } from "@/lib/query-client";

vi.mock("@/contexts/WebSocketContext", () => ({
  useWebSocketContext: () => ({ connected: true }),
}));
vi.mock("@/api/core/request", async () => {
  const { CancelablePromise } = await import("@/api/core/CancelablePromise");
  return {
    request: vi.fn(
      () =>
        new CancelablePromise((resolve) =>
          resolve({
            items: [],
            next_cursor: null,
            has_more: false,
          }),
        ),
    ),
  };
});

describe("artifact server-page requests", () => {
  const dispose: (() => void)[] = [];
  afterEach(() => {
    dispose.splice(0).forEach((release) => release());
    vi.clearAllMocks();
  });

  function provider() {
    const client = new QueryClient({
      defaultOptions: queryClient.getDefaultOptions(),
    });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    dispose.push(() => client.clear());
    return wrapper;
  }

  it("requests the selected execution page and exposes pagination metadata", async () => {
    const hook = renderHook(
      () =>
        useExecutionArtifacts(42, false, { cursor: "older-runs", perPage: 20 }),
      { wrapper: provider() },
    );
    dispose.unshift(hook.unmount);
    await waitFor(() => expect(hook.result.current.isSuccess).toBe(true));
    expect(request).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        url: "/api/v1/executions/{execution_id}/artifacts",
        path: { execution_id: 42 },
        query: { cursor: "older-runs", per_page: 20, type: undefined },
      }),
    );
    expect(hook.result.current.data?.has_more).toBe(false);
  });

  it("discovers progress with a filtered one-row request", async () => {
    const hook = renderHook(
      () => useExecutionArtifacts(42, true, { type: "progress", perPage: 1 }),
      { wrapper: provider() },
    );
    dispose.unshift(hook.unmount);
    await waitFor(() => expect(hook.result.current.isSuccess).toBe(true));
    expect(request).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        query: { cursor: undefined, per_page: 1, type: "progress" },
      }),
    );
  });

  it("requests a bounded history page instead of fetching and slicing all versions", async () => {
    const hook = renderHook(
      () =>
        useArtifactVersions(42, true, {
          cursor: "older-versions",
          perPage: 20,
        }),
      { wrapper: provider() },
    );
    dispose.unshift(hook.unmount);
    await waitFor(() => expect(hook.result.current.isSuccess).toBe(true));
    expect(request).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        url: "/api/v1/artifacts/{id}/versions",
        path: { id: 42 },
        query: { cursor: "older-versions", per_page: 20 },
      }),
    );
    expect(hook.result.current.data?.items).toEqual([]);
  });

  it("keeps a loaded paused page unchanged on shared invalidation but loads manual navigation", async () => {
    const client = new QueryClient({
      defaultOptions: queryClient.getDefaultOptions(),
    });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const hook = renderHook(
      () => {
        const [cursor, setCursor] = useState<string>();
        const query = useExecutionArtifacts(42, true, { cursor }, true);
        return { query, setCursor };
      },
      { wrapper },
    );
    dispose.unshift(() => {
      hook.unmount();
      client.clear();
    });
    await waitFor(() => expect(hook.result.current.query.isSuccess).toBe(true));
    expect(request).toHaveBeenCalledOnce();
    await act(() =>
      client.invalidateQueries({ queryKey: ["artifacts", "execution", 42] }),
    );
    expect(request).toHaveBeenCalledOnce();
    act(() => hook.result.current.setCursor("older-runs"));
    await waitFor(() => expect(hook.result.current.query.isSuccess).toBe(true));
    expect(request).toHaveBeenCalledTimes(2);
    expect(request).toHaveBeenLastCalledWith(
      expect.anything(),
      expect.objectContaining({
        query: { cursor: "older-runs", per_page: 20, type: undefined },
      }),
    );
  });
});
