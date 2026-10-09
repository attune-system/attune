import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import { RetentionService, type RetentionConfig } from "@/api/retention";
import {
  retentionKeys,
  useNativeMaintenanceStatus,
  useRetentionConfig,
  useUpdateRetentionConfig,
} from "./useRetentionConfig";

const config: RetentionConfig = {
  enabled: true,
  check_interval_seconds: 3600,
  batch_size: 1000,
  max_batches_per_target: 100,
  dry_run: false,
  advisory_lock_key: 7821001,
  targets: {
    events: {},
    enforcements: {},
    executions: {},
    execution_history: {},
    worker_history: {},
    sensor_process_history: {},
    audit_events: {},
    notifications: {},
    webhook_event_logs: {},
    inquiries: {},
    work_queue_items: {},
    work_queue_dispatches: {},
    pack_test_executions: {},
    execution_admission: {},
    workers: {},
    sensor_processes: {},
  },
  cache_retention: {
    enabled: true,
    max_cleanup_cycle_milliseconds: 30000,
    ddl_lock_timeout_milliseconds: 250,
    ddl_creation_statement_timeout_milliseconds: 5000,
    ddl_statement_timeout_milliseconds: 1000,
    statistics_interval_seconds: 300,
    statistics_statement_timeout_milliseconds: 5000,
    max_generations_per_cycle: 50,
    max_namespaces_per_cycle: 50,
    min_traversal_window_seconds: 3600,
    staging_expiry_seconds: 86400,
    dry_run: false,
    freshness_alerts_enabled: true,
    freshness_alert_grace_seconds: 900,
    staging_failure_alert_threshold: 3,
    alert_cooldown_seconds: 3600,
    alert_limit_per_cycle: 25,
  },
  native_maintenance: {
    enabled: true,
    partition_interval_seconds: 3600,
    summary_interval_seconds: 300,
    partition_lookahead_days: 7,
    max_partition_operations_per_cycle: 32,
    default_repair_row_limit: 1000,
    lock_timeout_milliseconds: 250,
    operation_timeout_milliseconds: 1000,
    max_partition_cycle_milliseconds: 5000,
    max_summary_buckets_per_cycle: 128,
    max_summary_invalidations_per_bucket: 10000,
    summary_bootstrap_hours: 24,
    max_summary_cycle_milliseconds: 5000,
  },
};

function harness() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
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

afterEach(() => vi.restoreAllMocks());

describe("retention hooks", () => {
  it("passes complete statistics settings through and refetches config and native status on save", async () => {
    const getConfig = vi
      .spyOn(RetentionService, "getRetentionConfig")
      .mockImplementation(
        () => new CancelablePromise((resolve) => resolve({ data: config })),
      );
    const getStatus = vi
      .spyOn(RetentionService, "getNativeMaintenanceStatus")
      .mockImplementation(
        () =>
          new CancelablePromise((resolve) =>
            resolve({
              data: {
                enabled: true,
                observed_at: "2026-10-07T12:00:00Z",
                partitions: [],
                summaries: [],
                schedule: [],
              },
            }),
          ),
      );
    const update = vi
      .spyOn(RetentionService, "updateRetentionConfig")
      .mockImplementation(
        ({ requestBody }) =>
          new CancelablePromise((resolve) => resolve({ data: requestBody })),
      );
    const { queryClient, wrapper } = harness();
    const { result, unmount } = renderHook(
      () => ({
        config: useRetentionConfig(),
        status: useNativeMaintenanceStatus(),
        update: useUpdateRetentionConfig(),
      }),
      { wrapper },
    );
    try {
      await waitFor(() =>
        expect(
          result.current.config.isSuccess && result.current.status.isSuccess,
        ).toBe(true),
      );
      expect(
        result.current.config.data?.data.cache_retention
          .statistics_interval_seconds,
      ).toBe(300);
      const edited: RetentionConfig = {
        ...config,
        cache_retention: {
          ...config.cache_retention,
          statistics_interval_seconds: 60,
          statistics_statement_timeout_milliseconds: 2000,
        },
      };
      await act(() => result.current.update.mutateAsync(edited));
      expect(update).toHaveBeenCalledWith({ requestBody: edited });
      await waitFor(() => {
        expect(getConfig).toHaveBeenCalledTimes(2);
        expect(getStatus).toHaveBeenCalledTimes(2);
      });
    } finally {
      unmount();
      queryClient.clear();
    }
  });

  it("does not invalidate saved configuration when an update is rejected", async () => {
    const rejected = new Error("invalid cache statistics interval");
    vi.spyOn(RetentionService, "updateRetentionConfig").mockImplementation(
      () => new CancelablePromise((_, reject) => reject(rejected)),
    );
    const { queryClient, wrapper } = harness();
    queryClient.setQueryData(retentionKeys.detail(), { data: config });
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const { result, unmount } = renderHook(() => useUpdateRetentionConfig(), {
      wrapper,
    });
    try {
      await act(async () => {
        await expect(result.current.mutateAsync(config)).rejects.toBe(rejected);
      });
      expect(invalidate).not.toHaveBeenCalled();
      expect(queryClient.getQueryData(retentionKeys.detail())).toEqual({
        data: config,
      });
    } finally {
      unmount();
      queryClient.clear();
    }
  });
});
