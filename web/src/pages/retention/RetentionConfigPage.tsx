import { useMemo, useState } from "react";
import { DatabaseZap, RotateCcw, Save } from "lucide-react";
import { useAuth } from "@/contexts/AuthContext";
import { hasPermission } from "@/lib/permissions";
import {
  retentionTargetKeys,
  retentionTargetLabels,
  type RetentionConfig,
  type CacheRetentionConfig,
  type NativeMaintenanceConfig,
  type RetentionTargetConfig,
  type RetentionTargetsConfig,
} from "@/api/retention";
import {
  useRetentionConfig,
  useUpdateRetentionConfig,
  useNativeMaintenanceStatus,
} from "@/hooks/useRetentionConfig";
import { NativeMaintenanceStatusPanel } from "./NativeMaintenanceStatusPanel";

type TargetField = keyof RetentionTargetsConfig;

const INPUT_CLASS =
  "w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500/30 disabled:bg-gray-100";

const cacheStorageFields = [
  {
    key: "max_cleanup_cycle_milliseconds",
    label: "Cache cleanup cycle budget in milliseconds",
    max: 3_600_000,
  },
  {
    key: "ddl_lock_timeout_milliseconds",
    label: "Cache DDL lock timeout in milliseconds",
    max: 3_600_000,
  },
  {
    key: "ddl_creation_statement_timeout_milliseconds",
    label: "Cache partition creation deadline in milliseconds",
    max: 3_600_000,
  },
  {
    key: "ddl_statement_timeout_milliseconds",
    label: "Cache partition cleanup deadline in milliseconds",
    max: 3_600_000,
  },
  {
    key: "statistics_interval_seconds",
    label: "Cache statistics interval in seconds",
    max: 86_400,
  },
  {
    key: "statistics_statement_timeout_milliseconds",
    label: "Cache statistics deadline in milliseconds",
    max: 3_600_000,
  },
] satisfies Array<{
  key: keyof CacheRetentionConfig;
  label: string;
  max: number;
}>;

type NativeNumericField = Exclude<keyof NativeMaintenanceConfig, "enabled">;
const nativeFields = [
  {
    key: "partition_interval_seconds",
    label: "Partition interval in seconds",
    advanced: false,
  },
  {
    key: "summary_interval_seconds",
    label: "Summary interval in seconds",
    advanced: false,
  },
  {
    key: "partition_lookahead_days",
    label: "Partition lookahead in days",
    advanced: false,
  },
  {
    key: "summary_bootstrap_hours",
    label: "Recent bootstrap hours",
    advanced: false,
  },
  {
    key: "max_partition_operations_per_cycle",
    label: "Partition operations per cycle",
    advanced: true,
  },
  {
    key: "default_repair_row_limit",
    label: "DEFAULT repair row limit",
    advanced: true,
  },
  {
    key: "lock_timeout_milliseconds",
    label: "Lock timeout in milliseconds",
    advanced: true,
  },
  {
    key: "operation_timeout_milliseconds",
    label: "Operation timeout in milliseconds",
    advanced: true,
  },
  {
    key: "max_partition_cycle_milliseconds",
    label: "Partition cycle budget in milliseconds",
    advanced: true,
  },
  {
    key: "max_summary_buckets_per_cycle",
    label: "Summary buckets per cycle",
    advanced: true,
  },
  {
    key: "max_summary_invalidations_per_bucket",
    label: "Invalidations per summary bucket",
    advanced: true,
  },
  {
    key: "max_summary_cycle_milliseconds",
    label: "Summary cycle budget in milliseconds",
    advanced: true,
  },
] satisfies Array<{
  key: NativeNumericField;
  label: string;
  advanced: boolean;
}>;

function secondsToDays(seconds: number | null | undefined): string {
  if (seconds == null) {
    return "";
  }
  return String(seconds / 86400);
}

function daysToSeconds(value: string): number | null {
  if (value.trim() === "") {
    return null;
  }
  const days = Number(value);
  if (!Number.isFinite(days) || days <= 0) {
    return 0;
  }
  return Math.round(days * 86400);
}

function formatRetention(value: number | null | undefined): string {
  if (value == null) {
    return "Forever";
  }
  const days = value / 86400;
  if (Number.isInteger(days)) {
    return `${days} day${days === 1 ? "" : "s"}`;
  }
  return `${value.toLocaleString()} seconds`;
}

function cloneConfig(config: RetentionConfig): RetentionConfig {
  return structuredClone(config);
}

export default function RetentionConfigPage() {
  const { user } = useAuth();
  const canUpdate = hasPermission(user, "retention", "update");
  const { data, dataUpdatedAt, isLoading, error } = useRetentionConfig();
  const updateRetention = useUpdateRetentionConfig();
  const nativeStatus = useNativeMaintenanceStatus();

  const loadedConfig = data?.data ?? null;

  if (error) {
    return (
      <div className="rounded-lg border border-red-200 bg-red-50 p-4 text-red-700">
        Failed to load retention configuration.
      </div>
    );
  }

  if (isLoading || !loadedConfig) {
    return (
      <div className="flex h-64 items-center justify-center">
        <div className="h-8 w-8 animate-spin rounded-full border-b-2 border-blue-600" />
      </div>
    );
  }

  return (
    <>
      <RetentionConfigEditor
        key={dataUpdatedAt}
        loadedConfig={loadedConfig}
        canUpdate={canUpdate}
        updateRetention={updateRetention}
      />
      <div className="px-6 pb-6" aria-live="polite">
        {nativeStatus.isError ? (
          <div className="rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700">
            Failed to load native maintenance status.
            <button
              type="button"
              onClick={() => void nativeStatus.refetch()}
              className="ml-3 underline"
            >
              Retry status
            </button>
          </div>
        ) : nativeStatus.data ? (
          <NativeMaintenanceStatusPanel status={nativeStatus.data.data} />
        ) : (
          <p className="text-sm text-gray-500">
            Loading native maintenance status...
          </p>
        )}
      </div>
    </>
  );
}

interface RetentionConfigEditorProps {
  loadedConfig: RetentionConfig;
  canUpdate: boolean;
  updateRetention: ReturnType<typeof useUpdateRetentionConfig>;
}

function retentionTargetDays(config: RetentionConfig): Record<string, string> {
  return Object.fromEntries(
    retentionTargetKeys.map((key) => [
      key,
      secondsToDays(config.targets[key].max_age_seconds),
    ]),
  );
}

function RetentionConfigEditor({
  loadedConfig,
  canUpdate,
  updateRetention,
}: RetentionConfigEditorProps) {
  const [draft, setDraft] = useState<RetentionConfig>(() =>
    cloneConfig(loadedConfig),
  );
  const [targetDays, setTargetDays] = useState<Record<string, string>>(() =>
    retentionTargetDays(loadedConfig),
  );

  const validationError = useMemo(() => {
    if (!draft) {
      return null;
    }
    if (draft.check_interval_seconds <= 0) {
      return "Check interval must be greater than zero.";
    }
    if (draft.batch_size <= 0) {
      return "Batch size must be greater than zero.";
    }
    for (const field of cacheStorageFields) {
      const value = draft.cache_retention[field.key];
      if (!Number.isSafeInteger(value) || value <= 0 || value > field.max) {
        return `${field.label} must be an integer between 1 and ${field.max}.`;
      }
    }
    for (const field of nativeFields) {
      const value = draft.native_maintenance[field.key];
      if (!Number.isSafeInteger(value) || value <= 0) {
        return `${field.label} must be a positive integer.`;
      }
      if (
        (field.key === "lock_timeout_milliseconds" ||
          field.key === "operation_timeout_milliseconds") &&
        value > 2147483647
      ) {
        return `${field.label} exceeds PostgreSQL's timeout range.`;
      }
    }
    if (
      !Number.isSafeInteger(draft.max_batches_per_target) ||
      draft.max_batches_per_target <= 0
    ) {
      return "Maximum batches per target must be a positive integer.";
    }
    for (const key of retentionTargetKeys) {
      const value = draft.targets[key].max_age_seconds;
      if (value === 0) {
        return `${retentionTargetLabels[key]} retention must be greater than zero days or blank for forever.`;
      }
    }
    return null;
  }, [draft]);

  const setGlobalField = <K extends keyof RetentionConfig>(
    key: K,
    value: RetentionConfig[K],
  ) => {
    setDraft((current) => (current ? { ...current, [key]: value } : current));
  };

  const setNativeField = <K extends keyof NativeMaintenanceConfig>(
    key: K,
    value: NativeMaintenanceConfig[K],
  ) => {
    setDraft((current) => ({
      ...current,
      native_maintenance: { ...current.native_maintenance, [key]: value },
    }));
  };

  const setCacheField = <K extends keyof CacheRetentionConfig>(
    key: K,
    value: CacheRetentionConfig[K],
  ) => {
    setDraft((current) => ({
      ...current,
      cache_retention: { ...current.cache_retention, [key]: value },
    }));
  };

  const nativeControl = (field: (typeof nativeFields)[number]) => (
    <label key={field.key} className="block">
      <span className="text-sm font-medium text-gray-700">{field.label}</span>
      <input
        type="number"
        min="1"
        step="1"
        value={draft.native_maintenance[field.key]}
        disabled={!canUpdate}
        onChange={(event) =>
          setNativeField(field.key, Number(event.target.value))
        }
        className={INPUT_CLASS}
      />
    </label>
  );

  const setTargetField = <K extends keyof RetentionTargetConfig>(
    target: TargetField,
    key: K,
    value: RetentionTargetConfig[K],
  ) => {
    setDraft((current) =>
      current
        ? {
            ...current,
            targets: {
              ...current.targets,
              [target]: {
                ...current.targets[target],
                [key]: value,
              },
            },
          }
        : current,
    );
  };

  const reset = () => {
    if (!loadedConfig) {
      return;
    }
    setDraft(cloneConfig(loadedConfig));
    setTargetDays(
      Object.fromEntries(
        retentionTargetKeys.map((key) => [
          key,
          secondsToDays(loadedConfig.targets[key].max_age_seconds),
        ]),
      ),
    );
  };

  const save = () => {
    if (!draft || validationError) {
      return;
    }
    updateRetention.mutate(draft);
  };

  return (
    <div className="p-6 space-y-6">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-3">
            <DatabaseZap className="h-8 w-8 text-blue-600" />
            <h1 className="text-3xl font-bold text-gray-900">
              Runtime retention
            </h1>
          </div>
          <p className="mt-2 max-w-3xl text-sm text-gray-600">
            Manage database retention for runtime metadata. Saved changes are
            persisted in PostgreSQL and picked up by the supervisor without
            restarting the service.
          </p>
        </div>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={reset}
            className="inline-flex items-center gap-2 rounded-md border border-gray-300 px-3 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50"
          >
            <RotateCcw className="h-4 w-4" />
            Reset
          </button>
          <button
            type="button"
            onClick={save}
            disabled={
              !canUpdate || !!validationError || updateRetention.isPending
            }
            className="inline-flex items-center gap-2 rounded-md bg-blue-600 px-3 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:bg-gray-400"
          >
            <Save className="h-4 w-4" />
            Save
          </button>
        </div>
      </div>

      {!canUpdate && (
        <div className="rounded-lg border border-amber-200 bg-amber-50 p-4 text-sm text-amber-800">
          You can view this configuration, but updating it requires the
          retention:update permission.
        </div>
      )}

      {validationError && (
        <div className="rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700">
          {validationError}
        </div>
      )}

      {updateRetention.isSuccess && (
        <div className="rounded-lg border border-green-200 bg-green-50 p-4 text-sm text-green-700">
          Retention configuration saved. The supervisor will use it on its next
          cycle.
        </div>
      )}

      {updateRetention.isError && (
        <div className="rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700">
          Failed to save retention configuration.
        </div>
      )}

      <section className="rounded-lg border border-gray-200 bg-white shadow-sm">
        <div className="border-b border-gray-200 px-6 py-4">
          <h2 className="text-lg font-semibold text-gray-900">
            Supervisor settings
          </h2>
          <p className="mt-1 text-sm text-gray-500">
            These settings control retention cycle cadence and safety behavior.
          </p>
        </div>
        <div className="grid gap-4 p-6 md:grid-cols-2 lg:grid-cols-3">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={draft.enabled}
              disabled={!canUpdate}
              onChange={(event) =>
                setGlobalField("enabled", event.target.checked)
              }
              className="h-4 w-4 rounded border-gray-300 text-blue-600"
            />
            <span className="text-sm font-medium text-gray-700">Enabled</span>
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={draft.dry_run}
              disabled={!canUpdate}
              onChange={(event) =>
                setGlobalField("dry_run", event.target.checked)
              }
              className="h-4 w-4 rounded border-gray-300 text-blue-600"
            />
            <span className="text-sm font-medium text-gray-700">Dry run</span>
          </label>
          <label className="block">
            <span className="text-sm font-medium text-gray-700">
              Check interval (seconds)
            </span>
            <input
              type="number"
              min="1"
              value={draft.check_interval_seconds}
              disabled={!canUpdate}
              onChange={(event) =>
                setGlobalField(
                  "check_interval_seconds",
                  Number(event.target.value),
                )
              }
              className={INPUT_CLASS}
            />
          </label>
          <label className="block">
            <span className="text-sm font-medium text-gray-700">
              Rows per batch
            </span>
            <input
              type="number"
              min="1"
              value={draft.batch_size}
              disabled={!canUpdate}
              onChange={(event) =>
                setGlobalField("batch_size", Number(event.target.value))
              }
              className={INPUT_CLASS}
            />
          </label>
          <label className="block">
            <span className="text-sm font-medium text-gray-700">
              Maximum batches per target
            </span>
            <input
              type="number"
              min="1"
              step="1"
              value={draft.max_batches_per_target}
              disabled={!canUpdate}
              onChange={(event) =>
                setGlobalField(
                  "max_batches_per_target",
                  Number(event.target.value),
                )
              }
              className={INPUT_CLASS}
            />
            <p className="mt-1 text-xs text-gray-500">
              Each target deletes at most rows per batch times maximum batches
              per cycle, then the supervisor continues with other maintenance.
            </p>
          </label>
          <label className="block">
            <span className="text-sm font-medium text-gray-700">
              Advisory lock key
            </span>
            <input
              type="number"
              value={draft.advisory_lock_key}
              disabled={!canUpdate}
              onChange={(event) =>
                setGlobalField("advisory_lock_key", Number(event.target.value))
              }
              className={INPUT_CLASS}
            />
          </label>
        </div>
      </section>

      <section className="rounded-lg border border-gray-200 bg-white shadow-sm">
        <div className="border-b border-gray-200 px-6 py-4">
          <h2 className="text-lg font-semibold text-gray-900">
            Native partition and summary maintenance
          </h2>
          <p className="mt-1 text-sm text-gray-500">
            Partition reconciliation and hourly summaries have independent
            cadences. Disabling these jobs keeps raw reads available. Summaries
            follow source retention.
          </p>
        </div>
        <div className="grid gap-4 p-6 md:grid-cols-2 lg:grid-cols-3">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={draft.native_maintenance.enabled}
              disabled={!canUpdate}
              onChange={(event) =>
                setNativeField("enabled", event.target.checked)
              }
              className="h-4 w-4 rounded border-gray-300 text-blue-600"
            />
            <span className="text-sm font-medium text-gray-700">
              Enable native maintenance
            </span>
          </label>
          {nativeFields.filter((field) => !field.advanced).map(nativeControl)}
        </div>
        <details className="border-t border-gray-200 px-6 py-4">
          <summary className="cursor-pointer text-sm font-medium text-gray-700">
            Advanced maintenance budgets
          </summary>
          <p className="mt-2 text-xs text-gray-500">
            DEFAULT repair moves a whole UTC day atomically. Days over the row
            limit remain in DEFAULT. Time budgets include lock waits;
            invalidation limits bound acknowledgement, not source row counts.
          </p>
          <div className="grid gap-4 pt-4 md:grid-cols-2 lg:grid-cols-3">
            {nativeFields.filter((field) => field.advanced).map(nativeControl)}
          </div>
        </details>
      </section>

      <section className="rounded-lg border border-gray-200 bg-white shadow-sm">
        <div className="border-b border-gray-200 px-6 py-4">
          <h2 className="text-lg font-semibold text-gray-900">
            Cache partition and statistics maintenance
          </h2>
          <p className="mt-1 text-sm text-gray-500">
            Creation, cleanup, and parent/leaf ANALYZE use separate deadlines.
            Statistics refresh runs in its own transaction when lifecycle
            changes are pending and its interval has elapsed. It is checked
            during the retention cycle, not by a separate timer.
          </p>
        </div>
        <div className="grid gap-4 p-6 md:grid-cols-2 lg:grid-cols-3">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={draft.cache_retention.enabled}
              disabled={!canUpdate}
              onChange={(event) =>
                setCacheField("enabled", event.target.checked)
              }
              className="h-4 w-4 rounded border-gray-300 text-blue-600"
            />
            <span className="text-sm font-medium text-gray-700">
              Enable cache maintenance
            </span>
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={draft.cache_retention.dry_run}
              disabled={!canUpdate}
              onChange={(event) =>
                setCacheField("dry_run", event.target.checked)
              }
              className="h-4 w-4 rounded border-gray-300 text-blue-600"
            />
            <span className="text-sm font-medium text-gray-700">
              Cache maintenance dry run
            </span>
          </label>
          {cacheStorageFields.map((field) => (
            <label key={field.key} className="block">
              <span className="text-sm font-medium text-gray-700">
                {field.label}
              </span>
              <input
                type="number"
                min="1"
                max={field.max}
                step="1"
                value={draft.cache_retention[field.key]}
                disabled={!canUpdate}
                onChange={(event) =>
                  setCacheField(field.key, Number(event.target.value))
                }
                className={INPUT_CLASS}
              />
            </label>
          ))}
        </div>
      </section>

      <section className="overflow-hidden rounded-lg border border-gray-200 bg-white shadow-sm">
        <div className="border-b border-gray-200 px-6 py-4">
          <h2 className="text-lg font-semibold text-gray-900">
            Retention targets
          </h2>
          <p className="mt-1 text-sm text-gray-500">
            Blank retention days keeps that target forever (no purging).
          </p>
        </div>
        <div className="overflow-x-auto">
          <table className="min-w-full divide-y divide-gray-200">
            <thead className="bg-gray-50">
              <tr>
                <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                  Target
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                  Retention days
                </th>
                <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
                  Effective retention
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-200 bg-white">
              {retentionTargetKeys.map((key) => (
                <tr key={key}>
                  <td className="whitespace-nowrap px-6 py-4 text-sm font-medium text-gray-900">
                    {retentionTargetLabels[key]}
                  </td>
                  <td className="px-6 py-4">
                    <input
                      type="number"
                      min="0.0001"
                      step="0.0001"
                      value={targetDays[key] ?? ""}
                      disabled={!canUpdate}
                      placeholder="Forever"
                      onChange={(event) => {
                        const value = event.target.value;
                        setTargetDays((current) => ({
                          ...current,
                          [key]: value,
                        }));
                        setTargetField(
                          key,
                          "max_age_seconds",
                          daysToSeconds(value),
                        );
                      }}
                      className={INPUT_CLASS}
                    />
                  </td>
                  <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-600">
                    {formatRetention(draft.targets[key].max_age_seconds)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
}
