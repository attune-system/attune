import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { NativeMaintenanceStatus, RetentionConfig } from "@/api/retention";
import { MaintenanceJob, ManagedTable, SummaryKind } from "@/api";
import RetentionConfigPage from "./RetentionConfigPage";

const mocks = vi.hoisted(() => ({
  mutate: vi.fn(),
  canUpdate: true,
  statusError: false,
  refetch: vi.fn(),
}));

vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ user: {} }) }));
vi.mock("@/lib/permissions", () => ({ hasPermission: () => mocks.canUpdate }));
vi.mock("@/hooks/useRetentionConfig", () => ({
  useRetentionConfig: () => ({
    data: { data: config },
    dataUpdatedAt: 1,
    isLoading: false,
  }),
  useUpdateRetentionConfig: () => ({ mutate: mocks.mutate, isPending: false }),
  useNativeMaintenanceStatus: () => ({
    data: { data: status },
    isError: mocks.statusError,
    refetch: mocks.refetch,
  }),
}));

// Deliberately non-default values prove the editor uses the persisted response.
const config: RetentionConfig = {
  enabled: true,
  check_interval_seconds: 1234,
  batch_size: 101,
  max_batches_per_target: 11,
  dry_run: false,
  advisory_lock_key: 123,
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
    max_cleanup_cycle_milliseconds: 30_000,
    ddl_lock_timeout_milliseconds: 250,
    ddl_creation_statement_timeout_milliseconds: 5_000,
    ddl_statement_timeout_milliseconds: 1_000,
    statistics_interval_seconds: 123,
    statistics_statement_timeout_milliseconds: 6_789,
    max_generations_per_cycle: 1,
    max_namespaces_per_cycle: 1,
    min_traversal_window_seconds: 1,
    staging_expiry_seconds: 1,
    dry_run: false,
    freshness_alerts_enabled: true,
    freshness_alert_grace_seconds: 1,
    staging_failure_alert_threshold: 1,
    alert_cooldown_seconds: 1,
    alert_limit_per_cycle: 1,
  },
  native_maintenance: {
    enabled: true,
    partition_interval_seconds: 1234,
    summary_interval_seconds: 123,
    partition_lookahead_days: 10,
    max_partition_operations_per_cycle: 9,
    default_repair_row_limit: 101,
    lock_timeout_milliseconds: 321,
    operation_timeout_milliseconds: 1500,
    max_partition_cycle_milliseconds: 7890,
    max_summary_buckets_per_cycle: 17,
    max_summary_invalidations_per_bucket: 45,
    summary_bootstrap_hours: 12,
    max_summary_cycle_milliseconds: 6789,
  },
};

const status: NativeMaintenanceStatus = {
  enabled: true,
  observed_at: "2026-10-06T12:00:00Z",
  partitions: [
    {
      parent: ManagedTable.EVENT,
      registered_partitions: 8,
      future_partitions: 7,
      missing_future_partitions: 3,
      default_rows_at_least: 102,
      default_count_exact: false,
      oldest_default_day: "2026-09-16T00:00:00Z",
    },
  ],
  summaries: [
    {
      kind: SummaryKind.EVENT_VOLUME,
      coverage_hours: 2,
      covered_since: "2026-10-06T08:00:00Z",
      covered_until: "2026-10-06T11:00:00Z",
      dirty_notifications: 4,
      dirty_hours: 1,
      oldest_dirty_bucket: "2026-10-06T09:00:00Z",
      oldest_notification: "2026-10-06T11:59:00Z",
      latest_success: null,
    },
  ],
  schedule: [
    {
      job: MaintenanceJob.SUMMARY,
      next_due: "2026-10-06T12:05:00Z",
      last_success: null,
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.canUpdate = true;
  mocks.statusError = false;
});

describe("native maintenance operator controls", () => {
  it("renders every persisted setting and saves edits without dropping other fields", () => {
    render(<RetentionConfigPage />);
    const section = screen
      .getByRole("heading", {
        name: "Native partition and summary maintenance",
      })
      .closest("section");
    if (!section) throw new Error("missing native settings section");
    const controls = within(section).getAllByRole("spinbutton", {
      hidden: true,
    });
    expect(controls).toHaveLength(
      Object.keys(config.native_maintenance).length - 1,
    );
    const expected = Object.entries(config.native_maintenance)
      .filter(([key]) => key !== "enabled")
      .map(([, value]) => value);
    expect(
      controls.map((control) => control.getAttribute("value")).sort(),
    ).toEqual(expected.map(String).sort());
    fireEvent.change(screen.getByLabelText("Partition lookahead in days"), {
      target: { value: "14" },
    });
    fireEvent.change(
      screen.getByLabelText("Invalidations per summary bucket"),
      { target: { value: "66" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(mocks.mutate).toHaveBeenCalledWith({
      ...config,
      native_maintenance: {
        ...config.native_maintenance,
        partition_lookahead_days: 14,
        max_summary_invalidations_per_bucket: 66,
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(screen.getByLabelText("Partition lookahead in days")).toHaveValue(
      10,
    );
  });

  it("blocks zero, fractional, unsafe and out-of-range native budgets", () => {
    render(<RetentionConfigPage />);
    for (const value of ["0", "1.5", "9007199254740992"]) {
      fireEvent.change(screen.getByLabelText("Partition lookahead in days"), {
        target: { value },
      });
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    }
    fireEvent.change(screen.getByLabelText("Partition lookahead in days"), {
      target: { value: "10" },
    });
    fireEvent.change(screen.getByLabelText("Lock timeout in milliseconds"), {
      target: { value: "2147483648" },
    });
    expect(
      screen.getByText(/exceeds PostgreSQL's timeout range/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(mocks.mutate).not.toHaveBeenCalled();
  });

  it("shows bounded DEFAULT counts, missing coverage, dirty backlog and next due times truthfully", () => {
    render(<RetentionConfigPage />);
    expect(screen.getByText("At least 102")).toBeInTheDocument();
    expect(screen.getByText("Missing future")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(screen.getByText("Dirty hours")).toBeInTheDocument();
    expect(screen.getByText("4")).toBeInTheDocument();
    expect(
      screen.getByText(/not proof of uninterrupted coverage/),
    ).toBeInTheDocument();
    expect(screen.getByText("2026-10-06T12:05:00Z")).toBeInTheDocument();
    expect(screen.getAllByText("None recorded")).toHaveLength(2);
  });

  it("keeps status visible for read-only users and disables mutation controls", () => {
    mocks.canUpdate = false;
    render(<RetentionConfigPage />);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.getByLabelText("Enable native maintenance")).toBeDisabled();
    expect(screen.getByLabelText("DEFAULT repair row limit")).toBeDisabled();
    expect(screen.getByText("At least 102")).toBeInTheDocument();
  });

  it("reports a status failure instead of presenting an empty or cached healthy status", () => {
    mocks.statusError = true;
    render(<RetentionConfigPage />);
    expect(
      screen.getByText("Failed to load native maintenance status."),
    ).toBeInTheDocument();
    expect(screen.queryByText("At least 102")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry status" }));
    expect(mocks.refetch).toHaveBeenCalledOnce();
  });
});

describe("cache storage maintenance controls", () => {
  it("loads and edits statistics independently of creation and cleanup deadlines", () => {
    render(<RetentionConfigPage />);
    expect(
      screen.getByLabelText("Cache statistics interval in seconds"),
    ).toHaveValue(123);
    expect(
      screen.getByLabelText("Cache statistics deadline in milliseconds"),
    ).toHaveValue(6789);
    fireEvent.change(
      screen.getByLabelText("Cache statistics interval in seconds"),
      {
        target: { value: "60" },
      },
    );
    fireEvent.change(
      screen.getByLabelText("Cache statistics deadline in milliseconds"),
      {
        target: { value: "2000" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(mocks.mutate).toHaveBeenCalledWith({
      ...config,
      cache_retention: {
        ...config.cache_retention,
        statistics_interval_seconds: 60,
        statistics_statement_timeout_milliseconds: 2000,
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(
      screen.getByLabelText("Cache statistics interval in seconds"),
    ).toHaveValue(123);
    expect(
      screen.getByLabelText("Cache statistics deadline in milliseconds"),
    ).toHaveValue(6789);
  });

  it.each([
    ["Cache statistics interval in seconds", "86400"],
    ["Cache statistics deadline in milliseconds", "3600000"],
  ])(
    "accepts both boundaries for %s even when cache maintenance is disabled",
    (label, max) => {
      render(<RetentionConfigPage />);
      fireEvent.click(screen.getByLabelText("Enable cache maintenance"));
      for (const value of ["1", max]) {
        fireEvent.change(screen.getByLabelText(label), { target: { value } });
        expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
      }
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      expect(mocks.mutate).toHaveBeenCalledWith(
        expect.objectContaining({
          cache_retention: expect.objectContaining({ enabled: false }),
        }),
      );
    },
  );

  it.each([
    ["Cache statistics interval in seconds", "86401"],
    ["Cache statistics deadline in milliseconds", "3600001"],
    ["Cache partition creation deadline in milliseconds", "3600001"],
    ["Cache partition cleanup deadline in milliseconds", "3600001"],
    ["Cache DDL lock timeout in milliseconds", "3600001"],
    ["Cache cleanup cycle budget in milliseconds", "3600001"],
  ])("rejects invalid values for %s", (label, overLimit) => {
    render(<RetentionConfigPage />);
    for (const value of ["0", "-1", "1.5", "", "9007199254740992", overLimit]) {
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    }
    expect(mocks.mutate).not.toHaveBeenCalled();
  });

  it("keeps cache controls read-only without retention:update", () => {
    mocks.canUpdate = false;
    render(<RetentionConfigPage />);
    expect(
      screen.getByLabelText("Cache statistics interval in seconds"),
    ).toBeDisabled();
    expect(
      screen.getByLabelText("Cache statistics deadline in milliseconds"),
    ).toBeDisabled();
    expect(screen.getByLabelText("Enable cache maintenance")).toBeDisabled();
    expect(screen.getByLabelText("Cache maintenance dry run")).toBeDisabled();
  });
});
