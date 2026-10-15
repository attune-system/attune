import {
  hashKey,
  type QueryClient,
  type QueryKey,
} from "@tanstack/react-query";

const COALESCE_MS = 500;
const schedulers = new WeakMap<QueryClient, ArtifactRefreshScheduler>();
const reconciliations = new WeakMap<QueryClient, ArtifactReconciliation>();
const alwaysActive = () => true;

interface PendingRefresh {
  key: QueryKey;
  requesters: Set<() => boolean>;
}

class ArtifactRefreshScheduler {
  private pending = new Map<string, PendingRefresh>();
  private waiting = new Map<string, PendingRefresh>();
  private timer: ReturnType<typeof setTimeout> | undefined;
  private unsubscribe: (() => void) | undefined;

  constructor(private client: QueryClient) {}

  enqueue(queryKey: QueryKey, requester = alwaysActive) {
    for (const query of this.client.getQueryCache().findAll({ queryKey })) {
      const pending = this.pending.get(query.queryHash) ?? {
        key: query.queryKey,
        requesters: new Set<() => boolean>(),
      };
      pending.requesters.add(requester);
      this.pending.set(query.queryHash, pending);
    }
    if (!this.pending.size) return;
    if (!this.unsubscribe) {
      this.unsubscribe = this.client.getQueryCache().subscribe((event) => {
        const waiting = this.waiting.get(event.query.queryHash);
        if (!waiting) return;
        if (event.type === "removed") {
          this.waiting.delete(event.query.queryHash);
          this.releaseIfIdle();
        } else if (event.query.state.fetchStatus === "idle") {
          this.waiting.delete(event.query.queryHash);
          for (const requester of waiting.requesters) {
            if (requester()) this.enqueue(waiting.key, requester);
          }
          this.releaseIfIdle();
        }
      });
    }
    this.timer ??= setTimeout(() => this.flush(), COALESCE_MS);
  }

  private flush() {
    this.timer = undefined;
    const pending = this.pending;
    this.pending = new Map();
    for (const [hash, refresh] of pending) {
      if (![...refresh.requesters].some((requester) => requester())) continue;
      const queryKey = refresh.key;
      const query = this.client.getQueryCache().find({ queryKey, exact: true });
      if (!query) continue;
      if (query.state.fetchStatus !== "idle") {
        const waiting = this.waiting.get(hash) ?? refresh;
        refresh.requesters.forEach((requester) =>
          waiting.requesters.add(requester),
        );
        this.waiting.set(hash, waiting);
        void this.client.invalidateQueries({
          queryKey,
          exact: true,
          refetchType: "none",
        });
      } else {
        void this.client.invalidateQueries(
          { queryKey, exact: true },
          { cancelRefetch: false },
        );
      }
    }
    this.releaseIfIdle();
  }

  private releaseIfIdle() {
    if (!this.pending.size && !this.waiting.size && this.timer === undefined) {
      this.unsubscribe?.();
      this.unsubscribe = undefined;
    }
  }
}

export function scheduleArtifactRefresh(
  client: QueryClient,
  queryKey: QueryKey,
  requester = alwaysActive,
) {
  let scheduler = schedulers.get(client);
  if (!scheduler) {
    scheduler = new ArtifactRefreshScheduler(client);
    schedulers.set(client, scheduler);
  }
  scheduler.enqueue(queryKey, requester);
}

interface ReconciliationSubscriber {
  key: QueryKey;
  interval: number;
  requester: () => boolean;
}

class ArtifactReconciliation {
  private subscribers = new Map<symbol, ReconciliationSubscriber>();
  private deadlines = new Map<string, number>();
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(private client: QueryClient) {}

  register(key: QueryKey, interval: number, requester: () => boolean) {
    const id = Symbol();
    this.subscribers.set(id, { key, interval, requester });
    const hash = hashKey(key);
    this.deadlines.set(
      hash,
      Math.min(this.deadlines.get(hash) ?? Infinity, Date.now() + interval),
    );
    this.timer ??= setInterval(() => {
      const now = Date.now();
      const groups = new Map<
        string,
        { interval: number; subscribers: ReconciliationSubscriber[] }
      >();
      for (const subscriber of this.subscribers.values()) {
        if (!subscriber.requester()) continue;
        const hash = hashKey(subscriber.key);
        const group = groups.get(hash) ?? {
          interval: subscriber.interval,
          subscribers: [],
        };
        group.interval = Math.min(group.interval, subscriber.interval);
        group.subscribers.push(subscriber);
        groups.set(hash, group);
      }
      for (const [hash, group] of groups) {
        if ((this.deadlines.get(hash) ?? Infinity) <= now) {
          this.deadlines.set(hash, now + group.interval);
          for (const subscriber of group.subscribers) {
            scheduleArtifactRefresh(
              this.client,
              subscriber.key,
              subscriber.requester,
            );
          }
        }
      }
    }, 1000);
    return () => {
      this.subscribers.delete(id);
      if (
        ![...this.subscribers.values()].some(
          (subscriber) => hashKey(subscriber.key) === hash,
        )
      ) {
        this.deadlines.delete(hash);
      }
      if (!this.subscribers.size) {
        clearInterval(this.timer);
        this.timer = undefined;
      }
    };
  }
}

export function registerArtifactReconciliation(
  client: QueryClient,
  key: QueryKey,
  interval: number,
  requester: () => boolean,
) {
  let reconciliation = reconciliations.get(client);
  if (!reconciliation) {
    reconciliation = new ArtifactReconciliation(client);
    reconciliations.set(client, reconciliation);
  }
  return reconciliation.register(key, interval, requester);
}
