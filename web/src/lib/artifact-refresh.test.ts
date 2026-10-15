import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  registerArtifactReconciliation,
  scheduleArtifactRefresh,
} from "./artifact-refresh";

describe("shared artifact refresh coordination", () => {
  const clients: QueryClient[] = [];
  const subscriptions: (() => void)[] = [];
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    subscriptions.splice(0).forEach((unsubscribe) => unsubscribe());
    clients.splice(0).forEach((client) => client.clear());
    vi.useRealTimers();
  });

  function activeQuery(
    key: readonly unknown[],
    queryFn: () => Promise<number>,
  ) {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, gcTime: Infinity } },
    });
    clients.push(client);
    const observer = new QueryObserver(client, {
      queryKey: key,
      queryFn,
      initialData: 0,
      staleTime: Infinity,
    });
    subscriptions.push(observer.subscribe(() => {}));
    return client;
  }

  it("coalesces overlapping prefixes and duplicate subscribers in a notification burst", async () => {
    const fetch = vi.fn(async () => 1);
    const client = activeQuery(["artifacts", 42, "versions"], fetch);
    for (let i = 0; i < 100; i++) {
      scheduleArtifactRefresh(client, ["artifacts", 42]);
      scheduleArtifactRefresh(client, ["artifacts", 42, "versions"]);
    }
    await vi.advanceTimersByTimeAsync(499);
    expect(fetch).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(fetch).toHaveBeenCalledOnce();
  });

  it("waits for an in-flight request and performs one trailing refresh", async () => {
    const deferred: { finish: (value: number) => void } = { finish: () => {} };
    const pending = new Promise<number>((resolve) => {
      deferred.finish = resolve;
    });
    const fetch = vi
      .fn()
      .mockImplementationOnce(() => pending)
      .mockResolvedValue(2);
    const key = ["artifacts", 42];
    const client = activeQuery(key, fetch);
    const initial = client.refetchQueries({ queryKey: key });
    for (let i = 0; i < 100; i++) scheduleArtifactRefresh(client, key);
    await vi.advanceTimersByTimeAsync(500);
    expect(fetch).toHaveBeenCalledOnce();
    deferred.finish(1);
    await initial;
    await vi.advanceTimersByTimeAsync(500);
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(client.getQueryData(key)).toBe(2);
  });

  it("does not fetch inactive queries or recreate a removed query", async () => {
    const client = new QueryClient();
    clients.push(client);
    client.setQueryData(["artifacts", 42], 1);
    client.setQueryData(["artifacts", 43], 1);
    scheduleArtifactRefresh(client, ["artifacts"]);
    client.removeQueries({ queryKey: ["artifacts", 43] });
    await vi.advanceTimersByTimeAsync(500);
    expect(client.getQueryState(["artifacts", 42])?.isInvalidated).toBe(true);
    expect(client.getQueryState(["artifacts", 43])).toBeUndefined();
  });

  it("keeps QueryClient instances independent", async () => {
    const first = vi.fn(async () => 1);
    const second = vi.fn(async () => 2);
    const key = ["artifacts", 42];
    const client = activeQuery(key, first);
    activeQuery(key, second);
    scheduleArtifactRefresh(client, key);
    await vi.advanceTimersByTimeAsync(500);
    expect(first).toHaveBeenCalledOnce();
    expect(second).not.toHaveBeenCalled();
  });

  it("drops queued work when its subscriber pauses or unmounts", async () => {
    const fetch = vi.fn(async () => 1);
    const key = ["artifacts", 42];
    const client = activeQuery(key, fetch);
    let active = true;
    scheduleArtifactRefresh(client, key, () => active);
    active = false;
    await vi.advanceTimersByTimeAsync(500);
    expect(fetch).not.toHaveBeenCalled();
    expect(client.getQueryState(key)?.isInvalidated).toBe(false);
  });

  it("reconciles a connected view without receiving a notification", async () => {
    const fetch = vi.fn(async () => 1);
    const key = ["artifacts", 42];
    const client = activeQuery(key, fetch);
    subscriptions.push(
      registerArtifactReconciliation(client, key, 30000, () => true),
    );
    await vi.advanceTimersByTimeAsync(30500);
    expect(fetch).toHaveBeenCalledOnce();
    expect(client.getQueryData(key)).toBe(1);
  });

  it("shares the reconciliation deadline across subscribers mounted at different times", async () => {
    const fetch = vi.fn(async () => 1);
    const key = ["artifacts", 42];
    const client = activeQuery(key, fetch);
    subscriptions.push(
      registerArtifactReconciliation(client, key, 30000, () => true),
    );
    await vi.advanceTimersByTimeAsync(10000);
    subscriptions.push(
      registerArtifactReconciliation(client, key, 30000, () => true),
    );
    await vi.advanceTimersByTimeAsync(20500);
    expect(fetch).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(10000);
    expect(fetch).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(20000);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("does not reconcile paused or unmounted subscribers", async () => {
    const fetch = vi.fn(async () => 1);
    const key = ["artifacts", 42];
    const client = activeQuery(key, fetch);
    let active = true;
    const release = registerArtifactReconciliation(
      client,
      key,
      3000,
      () => active,
    );
    active = false;
    await vi.advanceTimersByTimeAsync(10000);
    expect(fetch).not.toHaveBeenCalled();
    release();
    expect(vi.getTimerCount()).toBe(0);
  });
});
