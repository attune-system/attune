import { format as d3Format, scaleOrdinal } from "d3";
import { formatJsonValue } from "@/lib/format-utils";
import type { DashboardSourceResult } from "@/types/dashboard";

export type ChartRow = Record<string, unknown>;

export const CHART_PALETTE = [
  "#2563eb",
  "#059669",
  "#dc2626",
  "#7c3aed",
  "#ea580c",
  "#0891b2",
  "#4f46e5",
  "#65a30d",
  "#db2777",
  "#0f766e",
];

const LEVEL_COLORS: Record<string, string> = {
  // Generic health/severity
  good: "#16a34a",
  warning: "#f59e0b",
  bad: "#dc2626",
  ok: "#16a34a",
  error: "#dc2626",
  healthy: "#16a34a",
  degraded: "#f59e0b",
  unhealthy: "#dc2626",

  // Execution/workflow lifecycle
  completed: "#22c55e",
  succeeded: "#22c55e",
  failed: "#ef4444",
  timeout: "#f97316",
  running: "#3b82f6",
  requested: "#facc15",
  scheduling: "#fde047",
  scheduled: "#eab308",
  canceling: "#d1d5db",
  cancelled: "#9ca3af",
  canceled: "#9ca3af",
  abandoned: "#c084fc",

  // Worker/sensor lifecycle
  active: "#22c55e",
  busy: "#3b82f6",
  inactive: "#9ca3af",

  // Enforcement lifecycle
  created: "#3b82f6",
  processed: "#22c55e",
  disabled: "#9ca3af",
};

export function toRows(data: DashboardSourceResult["data"]): ChartRow[] {
  if (!data) return [];
  if (Array.isArray(data)) return data;
  return [data];
}

export function asNumber(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string") {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) {
      return parsed;
    }
  }
  return null;
}

export function toKey(value: unknown, fallback = "value"): string {
  if (value === null || value === undefined) return fallback;
  return formatJsonValue(value);
}

function formatDurationMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60000) return `${d3Format(".1f")(ms / 1000)} s`;
  if (ms < 3600000) return `${d3Format(".1f")(ms / 60000)} min`;
  return `${d3Format(".1f")(ms / 3600000)} h`;
}

function parseDateLike(value: unknown): Date | null {
  if (value instanceof Date && !Number.isNaN(value.valueOf())) {
    return value;
  }
  if (typeof value === "string" || typeof value === "number") {
    const parsed = new Date(value);
    if (!Number.isNaN(parsed.valueOf())) {
      return parsed;
    }
  }
  return null;
}

export function isInvalidTimeSinceValue(
  value: unknown,
  formatHint?: string,
): boolean {
  if (formatHint !== "time_since") {
    return false;
  }
  if (value === null || value === undefined) {
    return false;
  }
  return parseDateLike(value) === null;
}

function formatTimeSince(date: Date): string {
  const diffMs = date.getTime() - Date.now();
  const absMs = Math.abs(diffMs);

  let unit: Intl.RelativeTimeFormatUnit;
  let amount: number;

  if (absMs < 60_000) {
    unit = "second";
    amount = Math.round(diffMs / 1_000);
  } else if (absMs < 3_600_000) {
    unit = "minute";
    amount = Math.round(diffMs / 60_000);
  } else if (absMs < 86_400_000) {
    unit = "hour";
    amount = Math.round(diffMs / 3_600_000);
  } else if (absMs < 604_800_000) {
    unit = "day";
    amount = Math.round(diffMs / 86_400_000);
  } else if (absMs < 2_592_000_000) {
    unit = "week";
    amount = Math.round(diffMs / 604_800_000);
  } else if (absMs < 31_536_000_000) {
    unit = "month";
    amount = Math.round(diffMs / 2_592_000_000);
  } else {
    unit = "year";
    amount = Math.round(diffMs / 31_536_000_000);
  }

  return new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(
    amount,
    unit,
  );
}

function formatInvalidDateValue(value: unknown): string {
  if (typeof value === "string") {
    return `Invalid date value: ${value}`;
  }
  return `Invalid date value: ${JSON.stringify(value)}`;
}

export function formatTimeSinceParts(value: unknown): {
  relative: string;
  raw: string;
  invalid: boolean;
} {
  const date = parseDateLike(value);
  if (!date) {
    return {
      relative: formatInvalidDateValue(value),
      raw: "",
      invalid: true,
    };
  }

  return {
    relative: formatTimeSince(date),
    raw: typeof value === "string" ? value : date.toISOString(),
    invalid: false,
  };
}

function formatTimeSinceWithRawValue(value: unknown): string {
  const parts = formatTimeSinceParts(value);
  if (parts.invalid) {
    return parts.relative;
  }
  return `${parts.relative} (${parts.raw})`;
}

export function formatValue(
  value: unknown,
  formatHint?: string,
  unitHint?: string,
): string {
  if (value === null || value === undefined) return "—";

  if (formatHint === "time_since") {
    return formatTimeSinceWithRawValue(value);
  }

  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }

  const numeric = asNumber(value);
  const format = unitHint || formatHint;
  if (numeric !== null) {
    if (format === "percent") {
      return `${d3Format(".1f")(numeric)}%`;
    }
    if (format === "duration_ms") {
      return formatDurationMs(numeric);
    }
    if (format === "float") {
      return d3Format(",.2f")(numeric);
    }
    if (format === "integer" || format === "count") {
      return d3Format(",.0f")(numeric);
    }
    return d3Format(",.2~f")(numeric);
  }

  if (format === "relative_time") {
    const date = parseDateLike(value);
    if (date) {
      return date.toLocaleString();
    }
  }

  if (typeof value === "string") return value;

  return JSON.stringify(value);
}

export function getLevelColor(
  level: string | undefined,
  fallback = "#6b7280",
): string {
  if (!level) return fallback;
  const normalized = level.trim().toLowerCase();
  return LEVEL_COLORS[normalized] || fallback;
}

export interface CartesianSeriesModel {
  xDomain: string[];
  seriesKeys: string[];
  valuesBySeries: Map<string, Array<number | null>>;
  maxY: number;
}

export function buildCartesianSeriesModel(
  rows: ChartRow[],
  xField: string,
  yField: string,
  seriesField?: string,
): CartesianSeriesModel {
  const xDomain: string[] = [];
  const xIndex = new Map<string, number>();
  const seriesKeys: string[] = [];
  const seriesIndex = new Map<string, number>();
  const valuesBySeries = new Map<string, Array<number | null>>();

  for (const row of rows) {
    const xRaw = row[xField];
    if (xRaw === undefined || xRaw === null) continue;
    const x = toKey(xRaw);

    if (!xIndex.has(x)) {
      xIndex.set(x, xDomain.length);
      xDomain.push(x);
      for (const values of valuesBySeries.values()) {
        values.push(null);
      }
    }

    const seriesKey = seriesField ? toKey(row[seriesField]) : "value";
    if (!seriesIndex.has(seriesKey)) {
      seriesIndex.set(seriesKey, seriesKeys.length);
      seriesKeys.push(seriesKey);
      valuesBySeries.set(seriesKey, new Array(xDomain.length).fill(null));
    }

    const values = valuesBySeries.get(seriesKey);
    const xi = xIndex.get(x);
    const y = asNumber(row[yField]);
    if (!values || xi === undefined || y === null) continue;

    while (values.length < xDomain.length) {
      values.push(null);
    }
    values[xi] = values[xi] === null ? y : (values[xi] ?? 0) + y;
  }

  const maxY = Math.max(
    1,
    ...Array.from(valuesBySeries.values()).flatMap((series) =>
      series.filter((value): value is number => value !== null),
    ),
  );

  return {
    xDomain,
    seriesKeys,
    valuesBySeries,
    maxY,
  };
}

const STATUS_SERIES_COLORS: Record<string, string> = {
  completed: "#22c55e",
  succeeded: "#22c55e",
  failed: "#ef4444",
  timeout: "#f97316",
  running: "#3b82f6",
  requested: "#facc15",
  scheduling: "#fde047",
  scheduled: "#eab308",
  canceling: "#d1d5db",
  cancelled: "#9ca3af",
  canceled: "#9ca3af",
  abandoned: "#c084fc",
};

function statusSeriesColorForKey(key: string): string | null {
  const normalized = key.trim().toLowerCase();
  return STATUS_SERIES_COLORS[normalized] ?? null;
}

interface SeriesColorScaleOptions {
  preferStatusColors?: boolean;
}

export function createSeriesColorScale(
  seriesKeys: string[],
  options?: SeriesColorScaleOptions,
) {
  const preferStatusColors = options?.preferStatusColors ?? false;
  if (!preferStatusColors) {
    return scaleOrdinal<string, string>()
      .domain(seriesKeys)
      .range(CHART_PALETTE);
  }

  const usedColors = new Set<string>();
  const assignedColors = new Map<string, string>();

  for (const key of seriesKeys) {
    const statusColor = statusSeriesColorForKey(key);
    if (!statusColor) {
      continue;
    }
    assignedColors.set(key, statusColor);
    usedColors.add(statusColor);
  }

  let paletteCursor = 0;
  const paletteLength = CHART_PALETTE.length;

  for (const key of seriesKeys) {
    if (assignedColors.has(key)) {
      continue;
    }

    let selected = CHART_PALETTE[paletteCursor % paletteLength];
    let foundUnused = false;
    for (let step = 0; step < paletteLength; step += 1) {
      const candidate = CHART_PALETTE[(paletteCursor + step) % paletteLength];
      if (!usedColors.has(candidate)) {
        selected = candidate;
        paletteCursor = (paletteCursor + step + 1) % paletteLength;
        foundUnused = true;
        break;
      }
    }

    if (!foundUnused) {
      selected = CHART_PALETTE[paletteCursor % paletteLength];
      paletteCursor = (paletteCursor + 1) % paletteLength;
    }

    assignedColors.set(key, selected);
    usedColors.add(selected);
  }

  const range = seriesKeys.map(
    (key) => assignedColors.get(key) ?? CHART_PALETTE[0],
  );
  return scaleOrdinal<string, string>().domain(seriesKeys).range(range);
}

export function pickPreferredColumns(
  rows: ChartRow[],
  ordering: string[],
): string[] {
  const seen = new Set<string>();
  const columns: string[] = [];

  for (const col of ordering) {
    if (!seen.has(col) && rows.some((row) => col in row)) {
      seen.add(col);
      columns.push(col);
    }
  }

  for (const row of rows) {
    for (const key of Object.keys(row)) {
      if (!seen.has(key)) {
        seen.add(key);
        columns.push(key);
      }
    }
  }

  return columns;
}
