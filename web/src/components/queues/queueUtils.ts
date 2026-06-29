import {
  MUTABLE_PENDING_STATUSES,
  WorkQueueBatchMode,
  WorkQueueItemStatus,
  WorkQueueUpdateStrategy,
  type JsonValue,
  type WorkQueueResponse,
} from "@/api/queues";

export interface QueueTunableValue {
  source: "literal" | "pack_config" | "keystore";
  value?: JsonValue;
  path?: string;
  key_ref?: string;
  fallback?: JsonValue;
}

export interface QueueDispatchConfig {
  concurrency?: QueueTunableValue;
  batch_size?: QueueTunableValue;
  retry_limit?: number;
  inter_execution_delay_seconds?: number;
  coalescing?: QueueBatchCoalescingConfig;
}

export interface QueueAckContractConfig {
  version?: number;
}

export interface QueueBatchCoalescingConfig {
  enabled?: boolean;
  group_by_path?: string;
  across_priorities?: boolean;
}

export interface QueueConfig {
  dispatch?: QueueDispatchConfig;
  ack_contract?: QueueAckContractConfig;
}

export function prettyJson(value: unknown, fallback: unknown = {}): string {
  return JSON.stringify(value ?? fallback, null, 2);
}

export function parseJsonObject(
  label: string,
  raw: string,
): Record<string, JsonValue> {
  if (!raw.trim()) {
    throw new Error(`${label} is required`);
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(`${label} must be valid JSON`);
  }

  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${label} must be a JSON object`);
  }

  return parsed as Record<string, JsonValue>;
}

export function parseJsonValue(label: string, raw: string): JsonValue {
  if (!raw.trim()) {
    throw new Error(`${label} is required`);
  }

  try {
    return JSON.parse(raw) as JsonValue;
  } catch {
    throw new Error(`${label} must be valid JSON`);
  }
}

export function formatDateTime(value?: string | null): string {
  if (!value) {
    return "—";
  }
  return new Date(value).toLocaleString();
}

export function formatJsonPreview(value: JsonValue, maxLength = 80): string {
  const text = JSON.stringify(value);
  if (text.length <= maxLength) {
    return text;
  }
  return `${text.slice(0, maxLength - 1)}…`;
}

export function isMutablePendingStatus(status: WorkQueueItemStatus): boolean {
  return MUTABLE_PENDING_STATUSES.some((candidate) => candidate === status);
}

export function getQueueSourceBadge(isAdhoc: boolean) {
  if (isAdhoc) {
    return {
      label: "API-managed",
      classes: "bg-blue-100 text-blue-800",
      description: "Editable in the UI and managed by the API.",
    };
  }

  return {
    label: "Pack-managed",
    classes: "bg-purple-100 text-purple-800",
    description:
      "Read-only in the UI. Update the pack queue definition files instead.",
  };
}

export function getStatusBadge(status: WorkQueueItemStatus) {
  const map: Record<WorkQueueItemStatus, { label: string; classes: string }> = {
    [WorkQueueItemStatus.QUEUED]: {
      label: "Queued",
      classes: "bg-blue-100 text-blue-800",
    },
    [WorkQueueItemStatus.LEASED]: {
      label: "Leased",
      classes: "bg-amber-100 text-amber-800",
    },
    [WorkQueueItemStatus.RETRY]: {
      label: "Retry",
      classes: "bg-orange-100 text-orange-800",
    },
    [WorkQueueItemStatus.COMPLETED]: {
      label: "Completed",
      classes: "bg-green-100 text-green-800",
    },
    [WorkQueueItemStatus.FAILED]: {
      label: "Failed",
      classes: "bg-red-100 text-red-800",
    },
    [WorkQueueItemStatus.SKIPPED]: {
      label: "Skipped",
      classes: "bg-gray-100 text-gray-800",
    },
    [WorkQueueItemStatus.CANCELLED]: {
      label: "Cancelled",
      classes: "bg-slate-100 text-slate-800",
    },
  };

  return map[status];
}

export function getUpdateStrategyLabel(
  strategy: WorkQueueUpdateStrategy,
): string {
  switch (strategy) {
    case WorkQueueUpdateStrategy.IMMUTABLE:
      return "Immutable";
    case WorkQueueUpdateStrategy.MERGE_PATCH:
      return "Merge Patch";
    case WorkQueueUpdateStrategy.REPLACE:
    default:
      return "Replace";
  }
}

export function getBatchModeLabel(mode: WorkQueueBatchMode): string {
  return mode === WorkQueueBatchMode.BATCH ? "Batch" : "Single";
}

export function parseQueueConfig(
  value: JsonValue | null | undefined,
): QueueConfig {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return {};
  }
  return value as QueueConfig;
}

export function getQueueBatchCoalescingSummary(
  config: QueueConfig,
  batchMode: WorkQueueBatchMode,
): {
  enabled: boolean;
  groupByPath?: string;
  acrossPriorities: boolean;
  statusLabel: string;
} {
  const coalescing = config.dispatch?.coalescing;

  if (batchMode !== WorkQueueBatchMode.BATCH) {
    return {
      enabled: false,
      acrossPriorities: false,
      statusLabel: "Not applicable",
    };
  }

  if (!coalescing?.enabled) {
    return {
      enabled: false,
      acrossPriorities: false,
      groupByPath: coalescing?.group_by_path,
      statusLabel: "Disabled",
    };
  }

  return {
    enabled: true,
    groupByPath: coalescing.group_by_path,
    acrossPriorities: coalescing.across_priorities === true,
    statusLabel: "Enabled",
  };
}

export function formatQueueRetryLimit(value: number | undefined): string {
  const retryLimit = value ?? 0;
  return retryLimit === 1 ? "1 retry" : `${retryLimit} retries`;
}

function formatJsonInline(value: JsonValue | undefined): string {
  if (value === undefined) {
    return "unset";
  }

  if (
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean" ||
    value === null
  ) {
    return String(value);
  }

  return JSON.stringify(value);
}

export function formatQueueTunable(
  tunable: QueueTunableValue | undefined,
  fallback = "Default",
  resolvedValue?: number | null,
): string {
  if (!tunable) {
    return fallback;
  }

  switch (tunable.source) {
    case "literal":
      return `Literal: ${formatJsonInline(tunable.value)}`;
    case "pack_config":
      if (resolvedValue !== undefined && resolvedValue !== null) {
        return tunable.path
          ? `${resolvedValue} (Pack config: ${tunable.path})`
          : `${resolvedValue} (Pack config)`;
      }
      return tunable.path ? `Pack config: ${tunable.path}` : "Pack config";
    case "keystore":
      if (resolvedValue !== undefined && resolvedValue !== null) {
        return tunable.key_ref
          ? `${resolvedValue} (Keystore: ${tunable.key_ref}${tunable.path ? `.${tunable.path}` : ""})`
          : `${resolvedValue} (Keystore)`;
      }
      return tunable.key_ref
        ? `Keystore: ${tunable.key_ref}${tunable.path ? `.${tunable.path}` : ""}`
        : "Keystore";
    default:
      return fallback;
  }
}

export function resolveQueueTunableNumber(
  tunable: QueueTunableValue | undefined,
  resolvedValue?: number | null,
): number | null {
  if (resolvedValue !== undefined && resolvedValue !== null) {
    return resolvedValue;
  }

  if (tunable?.source !== "literal") {
    return null;
  }

  if (typeof tunable.value === "number" && Number.isFinite(tunable.value)) {
    return tunable.value;
  }

  if (typeof tunable.value === "string") {
    const parsed = Number(tunable.value);
    return Number.isFinite(parsed) ? parsed : null;
  }

  return null;
}

export function formatQueueInterExecutionDelay(
  delaySeconds: number | undefined,
  effectiveConcurrency: number | null,
): string {
  if (delaySeconds === undefined || delaySeconds <= 0) {
    return "Disabled";
  }

  if (effectiveConcurrency !== null && effectiveConcurrency !== 1) {
    return `${delaySeconds}s (inactive while concurrency = ${effectiveConcurrency})`;
  }

  return `${delaySeconds}s (after terminal execution)`;
}

export function getQueueItemSchemaSummary(
  itemSchema: JsonValue | null | undefined,
): string[] {
  if (
    !itemSchema ||
    typeof itemSchema !== "object" ||
    Array.isArray(itemSchema)
  ) {
    return ["No queue item schema defined."];
  }

  const entries = Object.entries(itemSchema as Record<string, JsonValue>);
  if (entries.length === 0) {
    return ["No queue item schema defined."];
  }

  return entries.map(([key, value]) => {
    const field =
      value && typeof value === "object" && !Array.isArray(value)
        ? (value as Record<string, JsonValue>)
        : {};
    const type = typeof field.type === "string" ? field.type : "any";
    const required = field.required === true ? " required" : "";
    return `${key}: ${type}${required}`;
  });
}

export function getQueueDispatchSummary(
  queue: Pick<WorkQueueResponse, "batch_mode" | "config">,
) {
  const config = parseQueueConfig(queue.config);
  return {
    concurrency: formatQueueTunable(config.dispatch?.concurrency, "Default: 1"),
    batchSize:
      queue.batch_mode === WorkQueueBatchMode.BATCH
        ? formatQueueTunable(config.dispatch?.batch_size, "Default: 1")
        : "Single item dispatch",
  };
}
