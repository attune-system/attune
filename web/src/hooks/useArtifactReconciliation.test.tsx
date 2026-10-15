import type { ReactNode } from "react";
import { act, renderHook } from "@testing-library/react";
import {
  QueryClient,
  QueryClientProvider,
  QueryObserver,
} from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useArtifactReconciliation } from "./useArtifactReconciliation";

const socket = vi.hoisted(() => ({ connected: true }));
vi.mock("@/contexts/WebSocketContext", () => ({
  useWebSocketContext: () => socket,
}));

describe("artifact reconciliation lifecycle", () => {
  const cleanup: (() => void)[] = [];
  beforeEach(() => {
    vi.useFakeTimers();
    socket.connected = true;
  });
  afterEach(() => {
    cleanup.splice(0).forEach((dispose) => dispose());
    vi.useRealTimers();
  });

  function view() {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, gcTime: Infinity } },
    });
    const fetch = vi.fn(async () => 1);
    const observer = new QueryObserver(client, {
      queryKey: ["artifacts", 42],
      queryFn: fetch,
      initialData: 0,
      staleTime: Infinity,
    });
    const unsubscribe = observer.subscribe(() => {});
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const hook = renderHook(
      ({ isRunning, paused }) =>
        useArtifactReconciliation({
          scope: "detail",
          id: 42,
          isRunning,
          paused,
        }),
      { wrapper, initialProps: { isRunning: true, paused: false } },
    );
    cleanup.push(() => {
      hook.unmount();
      unsubscribe();
      client.clear();
    });
    return { ...hook, fetch };
  }

  it("uses connected reconciliation rather than three-second polling", async () => {
    const { fetch } = view();
    await act(() => vi.advanceTimersByTimeAsync(3500));
    expect(fetch).not.toHaveBeenCalled();
    await act(() => vi.advanceTimersByTimeAsync(27000));
    expect(fetch).toHaveBeenCalledOnce();
  });

  it("falls back while disconnected and refreshes after reconnect", async () => {
    socket.connected = false;
    const { fetch, rerender } = view();
    await act(() => vi.advanceTimersByTimeAsync(3500));
    expect(fetch).toHaveBeenCalledOnce();
    socket.connected = true;
    rerender({ isRunning: true, paused: false });
    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("keeps completion paused and refreshes when the view resumes", async () => {
    const { fetch, rerender } = view();
    rerender({ isRunning: false, paused: true });
    await act(() => vi.advanceTimersByTimeAsync(60000));
    expect(fetch).not.toHaveBeenCalled();
    rerender({ isRunning: false, paused: false });
    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(fetch).toHaveBeenCalledOnce();
  });

  it("performs the final execution-completion refresh", async () => {
    const { fetch, rerender } = view();
    rerender({ isRunning: false, paused: false });
    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(fetch).toHaveBeenCalledOnce();
  });

  it(
    "recovers missed updates and unnotified deletion while the browser socket stays connected",
    async () => {
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: Infinity } },
      });
      const key = ["artifacts", "execution", 42];
      let serverRows = [1, 2];
      const fetch = vi.fn(async () => [...serverRows]);
      const observer = new QueryObserver(client, {
        queryKey: key,
        queryFn: fetch,
        initialData: [...serverRows],
        staleTime: Infinity,
      });
      const unsubscribe = observer.subscribe(() => {});
      const wrapper = ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      );
      const hook = renderHook(
        () => useArtifactReconciliation({ scope: "execution", id: 42, isRunning: true }),
        { wrapper },
      );
      cleanup.push(() => {
        hook.unmount();
        unsubscribe();
        client.clear();
      });

      // The server changes but no notification reaches the browser. A healthy
      // browser connection cannot distinguish any of these loss mechanisms.
      serverRows = [2, 3];
      await act(() => vi.advanceTimersByTimeAsync(29999));
      expect(client.getQueryData(key)).toEqual([1, 2]);
      expect(fetch).not.toHaveBeenCalled();
      await act(() => vi.advanceTimersByTimeAsync(501));
      expect(socket.connected).toBe(true);
      expect(fetch).toHaveBeenCalledOnce();
      expect(client.getQueryData(key)).toEqual([2, 3]);
    },
  );
});
